// Copyright (c) 2021 Intel Corporation
//
// SPDX-License-Identifier: BSD-2-Clause-Patent

#[macro_use]
extern crate clap;

use std::path::PathBuf;
use std::str::FromStr;
use std::{env, io, path::Path};

use clap::ArgAction;
use env_logger::Env;
use log::{error, trace, LevelFilter};
use ring::signature::{EcdsaKeyPair, RsaKeyPair, ECDSA_P384_SHA384_FIXED_SIGNING};
use td_layout::build_time::TD_SHIM_PAYLOAD_SIZE;
use td_shim_tools::signer::{PayloadSigner, SigningAlgorithm};
use td_shim_tools::{InputData, OutputFile};

const SIGNED_TDPAYLOAD_NAME: &str = "td-payload-signed";

fn main() -> io::Result<()> {
    let env = Env::default()
        .filter_or("MY_LOG_LEVEL", "info")
        .write_style_or("MY_LOG_STYLE", "always");
    env_logger::init_from_env(env);

    let matches = command!()
        .about("Sign shim payload with given private key")
        .arg(
            arg!([key] "private key file to sign the payload")
                .required(true)
        )
        .arg(
            arg!([payload] "payload binary file")
                .required(true)
        )
        .arg(
            arg!([ver] "payload version number")
                .required(true)
                .value_parser(value_parser!(u64)),
        )
        .arg(
            arg!([svn] "security version number")
                .required(true)
                .value_parser(value_parser!(u64)),
        )
        .arg(
            arg!(-A --algorithm "message signing algorithm: ['RSAPSS_3072_SHA384', 'ECDSA_NIST_P384_SHA384']")
                .required(false)
                .default_value("RSAPSS_3072_SHA384")
                .action(ArgAction::Set),
        )
        .arg(
            arg!(-l --"log-level" "logging level: [off, error, warn, info, debug, trace]")
                .required(false)
                .default_value("info")
                .action(ArgAction::Set),
        )
        .arg(
            arg!(-o --output "output of the signature file")
                .required(false)
                .value_parser(value_parser!(PathBuf))
                .action(ArgAction::Set),
        )
        .get_matches();

    if let Ok(lvl) = LevelFilter::from_str(matches.get_one::<String>("log-level").unwrap()) {
        log::set_max_level(lvl);
    }

    let payload_file = matches.get_one::<String>("payload").unwrap().as_str();
    let private_file = matches.get_one::<String>("key").unwrap().as_str();
    let version = matches.get_one::<u64>("ver").unwrap().clone();
    let svn = matches.get_one::<u64>("svn").unwrap().clone();
    let algorithm = matches.get_one::<String>("algorithm").unwrap().as_str();
    let output_file = match matches.get_one::<PathBuf>("output") {
        Some(v) => v.clone(),
        None => {
            let p = Path::new(payload_file).canonicalize().map_err(|e| {
                error!("Invalid payload file path {}: {}", payload_file, e);
                e
            })?;
            p.parent()
                .unwrap_or(Path::new("/"))
                .join(SIGNED_TDPAYLOAD_NAME)
        }
    };

    trace!(
        "td-shim-sign-payload {} {} {} {} {}",
        payload_file,
        version,
        svn,
        algorithm,
        private_file
    );

    let payload = InputData::new(payload_file, 0..=TD_SHIM_PAYLOAD_SIZE as usize, "payload")?;
    let mut private = InputData::new(private_file, 0..=1024 * 1024, "private key")?;
    let algorithm = match algorithm {
        "RSAPSS_3072_SHA384" => {
            let rsa_key_pair = RsaKeyPair::from_pkcs8(private.as_bytes()).map_err(|e| {
                error!("Can not load RSA private key from {}: {}", private_file, e);
                io::Error::new(io::ErrorKind::Other, "Can not load RSA private key")
            })?;
            SigningAlgorithm::Rsapss3072Sha384(rsa_key_pair)
        }
        "ECDSA_NIST_P384_SHA384" => {
            let ecdsa_key_pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P384_SHA384_FIXED_SIGNING, private.as_bytes())
                    .map_err(|e| {
                        error!("Can not load DSA private key from {}: {}", private_file, e);
                        io::Error::new(io::ErrorKind::Other, "Can not load DSA private key")
                    })?;
            SigningAlgorithm::EcdsaNistP384Sha384(ecdsa_key_pair)
        }
        _ => {
            error!("Unsupported signing algorithm: {}", algorithm);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Unsupported signing algorithm",
            ));
        }
    };

    // 1) Generate the verify header and write into the start of signed image
    // 2) Sign the data(verify header | payload binary)
    // 3) Put the public key bytes and signature at the end of the signed imgae.
    let mut signer = PayloadSigner::new(payload.as_bytes(), algorithm);
    let header = signer.build_header(version, svn);
    let signed_image = signer.sign(header)?;

    // Clear the private key memory.
    private.clear();

    // Create and write the signed payload image.
    let mut output = OutputFile::new(output_file)?;
    output.seek_and_write(0, signed_image, "signed payload")?;
    output.flush()?;

    Ok(())
}
