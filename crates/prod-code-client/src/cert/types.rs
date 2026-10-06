/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum CertCommands {
    /// Initialize a full cluster PKI hierarchy: generates Root CA, issues a node certificate/key, and calculates the cert pin.
    Init {
        /// Directory where ca.crt, ca.key, node.crt, and node.key will be written.
        #[arg(short, long, default_value = "certs")]
        out_dir: PathBuf,
        /// Expected TLS server name (SNI/SAN DNS). Defaults to prod-code.internal.
        #[arg(long)]
        server_name: Option<String>,
        /// Extra IP addresses to include in the certificate SAN list (repeatable).
        #[arg(long = "ip")]
        ips: Vec<String>,
        /// Output summary report as JSON.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Generate only a Root Certificate Authority (ca.crt and ca.key with 0600 permissions).
    Ca {
        /// Directory where ca.crt and ca.key will be written.
        #[arg(short, long, default_value = "certs")]
        out_dir: PathBuf,
        /// Common name for the CA. Defaults to "prod-code Cluster Root CA".
        #[arg(long, default_value = "prod-code Cluster Root CA")]
        common_name: String,
    },
    /// Issue a node certificate and private key signed by an existing Root CA.
    Node {
        /// Path to CA certificate (PEM).
        #[arg(long, default_value = "certs/ca.crt")]
        ca_cert: PathBuf,
        /// Path to CA private key (PEM).
        #[arg(long, default_value = "certs/ca.key")]
        ca_key: PathBuf,
        /// Directory where node.crt and node.key will be written.
        #[arg(short, long, default_value = "certs")]
        out_dir: PathBuf,
        /// Prefix for certificate and key file names (default: node).
        #[arg(long, default_value = "node")]
        prefix: String,
        /// DNS names to include in SAN list (repeatable).
        #[arg(long = "dns")]
        dns: Vec<String>,
        /// IP addresses to include in SAN list (repeatable).
        #[arg(long = "ip")]
        ips: Vec<String>,
    },
    /// Compute and print the SHA-256 certificate pin (lowercase hex) for a certificate.
    Pin {
        /// Path to the certificate file (PEM).
        cert_file: PathBuf,
    },
    /// Verify certificate validity, file permissions, and optional CA chain or pin match.
    Verify {
        /// Path to the certificate file (PEM).
        cert_file: PathBuf,
        /// Optional path to the private key file (PEM) to verify matching and permissions.
        #[arg(long)]
        key_file: Option<PathBuf>,
        /// Optional path to CA certificate (PEM) for trust validation.
        #[arg(long)]
        ca_cert: Option<PathBuf>,
        /// Optional server name (DNS or IP) expected in certificate SAN during CA trust verification.
        #[arg(long)]
        server_name: Option<String>,
        /// Optional expected SHA-256 certificate pin.
        #[arg(long)]
        pin: Option<String>,
    },
}
