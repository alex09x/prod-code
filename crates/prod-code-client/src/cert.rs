//! Cluster PKI, certificate authority, node certificates, pinning, and TLS bootstrap (Phase 5.6).
//!
//! Provides CLI commands to generate self-signed Root CAs, issue gateway/node certificates,
//! calculate certificate pins, verify certificate validity and key permissions, and output
//! setup environment variables for strict TLS cluster deployments.

use anyhow::{Context, Result, bail};
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

pub async fn run_cert(cmd: CertCommands) -> Result<()> {
    match cmd {
        CertCommands::Init {
            out_dir,
            server_name,
            ips,
            json,
        } => {
            let mut parsed_ips = Vec::new();
            for ip_str in ips {
                let ip: std::net::IpAddr = ip_str
                    .parse()
                    .with_context(|| format!("invalid IP address '{ip_str}'"))?;
                parsed_ips.push(ip);
            }
            let report = prod_code_protocol::tls::pki::init_cluster_pki(
                &out_dir,
                server_name.as_deref(),
                &parsed_ips,
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "prod-code cluster PKI initialized successfully in {}:",
                    out_dir.display()
                );
                println!("  Root CA Certificate:  {}", report.ca_cert_path.display());
                println!(
                    "  Root CA Private Key:  {} (mode 0600)",
                    report.ca_key_path.display()
                );
                println!("  Node Certificate:     {}", report.node_cert_path.display());
                println!(
                    "  Node Private Key:     {} (mode 0600)",
                    report.node_key_path.display()
                );
                println!("  SHA-256 Cert Pin:     {}", report.cert_pin);
                println!("  TLS Server Name:      {}", report.server_name);
                println!();
                println!("To activate strict TLS on cluster nodes and clients:");
                println!("{}", report.env_example);
            }
            Ok(())
        }
        CertCommands::Ca {
            out_dir,
            common_name,
        } => {
            let (ca_cert, ca_key) = prod_code_protocol::tls::pki::generate_ca(&common_name)?;
            let (cert_p, key_p) = prod_code_protocol::tls::pki::write_cert_and_key(
                &out_dir, "ca", &ca_cert, &ca_key,
            )?;
            println!("Root CA generated in {}:", out_dir.display());
            println!("  CA Certificate: {}", cert_p.display());
            println!("  CA Private Key: {} (mode 0600)", key_p.display());
            Ok(())
        }
        CertCommands::Node {
            ca_cert,
            ca_key,
            out_dir,
            prefix,
            dns,
            ips,
        } => {
            let ca_cert_pem = std::fs::read_to_string(&ca_cert).with_context(|| {
                format!("failed reading CA certificate from {}", ca_cert.display())
            })?;
            let ca_key_pem = std::fs::read_to_string(&ca_key).with_context(|| {
                format!("failed reading CA private key from {}", ca_key.display())
            })?;
            let mut parsed_ips = Vec::new();
            for ip_str in ips {
                let ip: std::net::IpAddr = ip_str
                    .parse()
                    .with_context(|| format!("invalid IP address '{ip_str}'"))?;
                parsed_ips.push(ip);
            }
            let san_names = if dns.is_empty() {
                vec![
                    prod_code_protocol::tls::DEFAULT_TLS_SERVER_NAME.to_string(),
                    "localhost".to_string(),
                ]
            } else {
                dns
            };
            let (node_cert, node_key) = prod_code_protocol::tls::pki::generate_node_cert(
                &ca_cert_pem,
                &ca_key_pem,
                &san_names,
                &parsed_ips,
            )?;
            let (cert_p, key_p) = prod_code_protocol::tls::pki::write_cert_and_key(
                &out_dir, &prefix, &node_cert, &node_key,
            )?;
            let pin = prod_code_protocol::tls::pki::compute_cert_pin(node_cert.as_bytes())?;
            println!("Node certificate issued in {}:", out_dir.display());
            println!("  Certificate:      {}", cert_p.display());
            println!("  Private Key:      {} (mode 0600)", key_p.display());
            println!("  SHA-256 Cert Pin: {}", pin);
            Ok(())
        }
        CertCommands::Pin { cert_file } => {
            let bytes = std::fs::read(&cert_file).with_context(|| {
                format!("failed reading certificate from {}", cert_file.display())
            })?;
            let pin = prod_code_protocol::tls::pki::compute_cert_pin(&bytes)?;
            println!("{pin}");
            Ok(())
        }
        CertCommands::Verify {
            cert_file,
            key_file,
            ca_cert,
            server_name,
            pin,
        } => {
            let cert_bytes = std::fs::read(&cert_file).with_context(|| {
                format!("failed reading certificate from {}", cert_file.display())
            })?;
            let cert_pin = prod_code_protocol::tls::pki::compute_cert_pin(&cert_bytes)?;
            let certs = prod_code_protocol::tls::load_certs(&cert_file)
                .with_context(|| format!("failed parsing certificate at {}", cert_file.display()))?;
            let first_cert = certs.first().context("no certificate found in file")?;

            println!("Certificate: {}", cert_file.display());
            println!("  SHA-256 Pin: {}", cert_pin);

            if let Some(expected_pin) = pin {
                let norm_expected = expected_pin.replace([':', ' '], "").to_ascii_lowercase();
                if cert_pin == norm_expected {
                    println!("  [OK] SHA-256 certificate pin matches expected pin.");
                } else {
                    bail!("SHA-256 pin mismatch: expected {norm_expected}, got {cert_pin}");
                }
            }

            if let Some(ca_path) = ca_cert {
                let ca_certs = prod_code_protocol::tls::load_certs(&ca_path)
                    .with_context(|| format!("failed loading CA certs from {}", ca_path.display()))?;
                println!("  CA Certificate: {}", ca_path.display());
                
                prod_code_protocol::tls::pki::verify_cert_against_ca(
                    first_cert,
                    if certs.len() > 1 { &certs[1..] } else { &[] },
                    &ca_certs,
                    server_name.as_deref(),
                ).with_context(|| {
                    format!("certificate trust verification failed against CA {}", ca_path.display())
                })?;
                println!("  [OK] Certificate trust chain and validity period verified against CA.");
            }

            if let Some(key_path) = key_file {
                prod_code_protocol::tls::check_key_permissions(&key_path).with_context(|| {
                    format!("insecure private key permissions on {}", key_path.display())
                })?;
                let _ = prod_code_protocol::tls::load_private_key(&key_path)
                    .with_context(|| format!("failed loading private key at {}", key_path.display()))?;
                println!("  Private Key: {}", key_path.display());
                println!("  [OK] Private key permissions (0600) and format verified.");
            }

            println!("Verification passed.");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cert_cli_init_ca_node_pin_and_verify_cycle() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join("tls");

        // 1. Run Init
        run_cert(CertCommands::Init {
            out_dir: out_dir.clone(),
            server_name: Some("cluster.internal".to_string()),
            ips: vec!["10.0.0.5".to_string()],
            json: true,
        })
        .await
        .unwrap();

        let ca_crt = out_dir.join("ca.crt");
        let ca_key = out_dir.join("ca.key");
        let node_crt = out_dir.join("node.crt");
        let node_key = out_dir.join("node.key");

        assert!(ca_crt.exists());
        assert!(ca_key.exists());
        assert!(node_crt.exists());
        assert!(node_key.exists());

        // 2. Run Pin
        let cert_bytes = std::fs::read(&node_crt).unwrap();
        let expected_pin = prod_code_protocol::tls::pki::compute_cert_pin(&cert_bytes).unwrap();
        run_cert(CertCommands::Pin {
            cert_file: node_crt.clone(),
        })
        .await
        .unwrap();

        // 3. Run Verify with correct pin and key
        run_cert(CertCommands::Verify {
            cert_file: node_crt.clone(),
            key_file: Some(node_key.clone()),
            ca_cert: Some(ca_crt.clone()),
            server_name: Some("cluster.internal".to_string()),
            pin: Some(expected_pin.clone()),
        })
        .await
        .unwrap();

        // 4. Verify with mismatched pin fails
        let bad_pin = "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        assert!(
            run_cert(CertCommands::Verify {
                cert_file: node_crt.clone(),
                key_file: Some(node_key.clone()),
                ca_cert: Some(ca_crt.clone()),
                server_name: None,
                pin: Some(bad_pin),
            })
            .await
            .is_err()
        );

        // 5. Issue additional node cert using Ca and Node commands
        let client_dir = temp.path().join("client_tls");
        run_cert(CertCommands::Node {
            ca_cert: ca_crt.clone(),
            ca_key: ca_key.clone(),
            out_dir: client_dir.clone(),
            prefix: "client".to_string(),
            dns: vec!["client.internal".to_string()],
            ips: vec!["127.0.0.1".to_string()],
        })
        .await
        .unwrap();

        let client_crt = client_dir.join("client.crt");
        let client_key = client_dir.join("client.key");
        assert!(client_crt.exists());
        assert!(client_key.exists());

        run_cert(CertCommands::Verify {
            cert_file: client_crt,
            key_file: Some(client_key),
            ca_cert: Some(ca_crt.clone()),
            server_name: Some("client.internal".to_string()),
            pin: None,
        })
        .await
        .unwrap();

        // 6. Verify against unrelated untrusted CA fails
        let fake_ca_dir = temp.path().join("fake_ca");
        run_cert(CertCommands::Ca {
            out_dir: fake_ca_dir.clone(),
            common_name: "Unrelated Impostor CA".to_string(),
        })
        .await
        .unwrap();
        let fake_ca_crt = fake_ca_dir.join("ca.crt");
        assert!(
            run_cert(CertCommands::Verify {
                cert_file: node_crt.clone(),
                key_file: None,
                ca_cert: Some(fake_ca_crt),
                server_name: None,
                pin: None,
            })
            .await
            .is_err(),
            "verification against untrusted CA must fail"
        );
    }
}

