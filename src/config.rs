use anyhow::{Result, anyhow};
use clap::{Parser, value_parser};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "my-cont-srv")]
pub struct Cli {
    #[arg(
        short,
        long,
        default_value = "0.0.0.0",
        help = "The address the server bind to. Specify '::' to bind to all addresses"
    )]
    address: String,

    #[arg(short, long, default_value_t = 1131, value_parser = value_parser!(u16).range(1..),
        help = "The server listening port")]
    port: u16,

    #[arg(short, long, default_value = ".", help = "The contents root directory")]
    root_dir: PathBuf,

    #[arg(short, long, help = "The path of the config file with toml format")]
    config_file: Option<PathBuf>,

    #[arg(
        long,
        help = "Hash the password and exit after printing the result. The hash can be used in the config file."
    )]
    pub hash_password: Option<String>,
}

#[derive(Deserialize, Debug)]
struct TomlConfig {
    pub address: Option<String>,
    pub port: Option<u16>,
    pub root_dir: Option<PathBuf>,
    pub cert_path: Option<PathBuf>,
    pub key_path: Option<PathBuf>,
    pub user_name: Option<String>,
    pub password_hash: Option<String>,
    pub workers: Option<usize>,
}

pub struct Config {
    pub address: String,
    pub port: u16,
    pub root_dir: PathBuf,
    pub cert_path: Option<PathBuf>,
    pub key_path: Option<PathBuf>,
    pub user_name: Option<String>,
    pub password_hash: Option<String>,
    pub workers: usize,
}

#[inline]
pub fn parse_cli() -> Cli {
    Cli::parse()
}

#[inline]
pub fn parse_cli_from(args: Vec<String>) -> Cli {
    Cli::parse_from(args)
}

fn parse_config_file(file: &PathBuf) -> Result<TomlConfig> {
    let config_str = std::fs::read_to_string(file)?;
    let config: TomlConfig = toml::from_str(&config_str)?;
    Ok(config)
}

pub fn get_config(cli: Cli) -> Result<Config> {
    let mut config = Config {
        port: cli.port,
        root_dir: cli.root_dir,
        address: cli.address,
        cert_path: None,
        key_path: None,
        user_name: None,
        password_hash: None,
        workers: 2,
    };

    if let Some(path) = cli.config_file {
        let toml_cfg = parse_config_file(&path)?;
        if (toml_cfg.cert_path.is_some() && toml_cfg.key_path.is_none())
            || (toml_cfg.cert_path.is_none() && toml_cfg.key_path.is_some())
        {
            eprintln!("Both cert file and key file are needed for HTTPS support!");
            return Err(anyhow!("Missing cert file or key file"));
        }
        if (toml_cfg.user_name.is_some() && toml_cfg.password_hash.is_none())
            || (toml_cfg.user_name.is_none() && toml_cfg.password_hash.is_some())
        {
            eprintln!("Both user name and password hash are needed for user authentication!");
            return Err(anyhow!("Missing user name or password hash"));
        }
        if let Some(address) = toml_cfg.address {
            config.address = address;
        }
        if let Some(port) = toml_cfg.port {
            config.port = port;
        }
        if let Some(dir) = toml_cfg.root_dir {
            config.root_dir = dir;
        }
        if let Some(workers) = toml_cfg.workers {
            config.workers = workers;
        }
        config.cert_path = toml_cfg.cert_path;
        config.key_path = toml_cfg.key_path;
        config.user_name = toml_cfg.user_name;
        config.password_hash = toml_cfg.password_hash;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppState, AuthInfo, app_config, basic_auth};
    use ::base64::Engine;
    use actix_http::StatusCode;
    use actix_web::{App, middleware::Condition, test, web};
    use actix_web_httpauth::middleware::HttpAuthentication;
    use base64::engine::general_purpose as base64;
    use std::time::Instant;

    fn args_to_vec(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    async fn test_cli() {
        let cli = Cli::parse_from(Vec::<String>::new());
        assert_eq!(cli.port, 1131);
        assert_eq!(cli.root_dir, PathBuf::from("."));
        assert_eq!(cli.config_file, None);
        assert_eq!(cli.hash_password, None);
        let cli = Cli::parse_from(args_to_vec(&[
            "my-cont-srv",
            "-a",
            "123.123.123.123",
            "-p",
            "1139",
            "-r",
            "./目录",
            "-c",
            "配置文件.toml",
            "--hash-password",
            "mypassword",
        ]));
        assert_eq!(cli.address, String::from("123.123.123.123"));
        assert_eq!(cli.port, 1139);
        assert_eq!(cli.root_dir, PathBuf::from("./目录"));
        assert_eq!(cli.config_file, Some(PathBuf::from("配置文件.toml")));
        assert_eq!(cli.hash_password, Some(String::from("mypassword")));
    }

    #[test]
    async fn test_config() {
        let cli = Cli::parse_from(args_to_vec(&["my-cont-srv", "-c", "res_dir/config.toml"]));
        let cfg = get_config(cli);
        assert!(cfg.is_ok());
        let cfg = cfg.unwrap();
        assert_eq!(cfg.address, String::from("127.0.0.1"));
        assert_eq!(cfg.port, 11310);
        assert_eq!(cfg.root_dir, PathBuf::from("res_dir"));
        assert_eq!(cfg.cert_path, Some(PathBuf::from("res_dir/cert.pem")));
        assert_eq!(cfg.key_path, Some(PathBuf::from("res_dir/key.pem")));
        assert_eq!(cfg.user_name, Some(String::from("myuser")));
        assert_eq!(
            cfg.password_hash,
            Some(String::from(
                "$2b$12$iNwN4yF3d9AUXBOexcfpDuBG2GH25Wmz9XGPf5q73Dio5cK6GHvWi"
            ))
        );
        assert_eq!(cfg.workers, 3usize);
    }

    #[actix_web::test]
    async fn test_auth() {
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let auth_info = AuthInfo::new(
            "myuser",
            "$2b$12$iNwN4yF3d9AUXBOexcfpDuBG2GH25Wmz9XGPf5q73Dio5cK6GHvWi",
        );
        let app = test::init_service(
            App::new()
                .configure(app_config)
                .app_data(app_data)
                .app_data(auth_info)
                .wrap(Condition::new(true, HttpAuthentication::basic(basic_auth))),
        )
        .await;
        // no auth info
        let req = test::TestRequest::default().to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // wrong user name
        let req = test::TestRequest::default()
            .append_header((
                "Authorization",
                format!("Basic {}", base64::STANDARD.encode("dummy:mypassword")),
            ))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // wrong password
        let req = test::TestRequest::default()
            .append_header((
                "Authorization",
                format!("Basic {}", base64::STANDARD.encode("myuser:dummy")),
            ))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // correct auth info
        let req = test::TestRequest::default()
            .append_header((
                "Authorization",
                format!("Basic {}", base64::STANDARD.encode("myuser:mypassword")),
            ))
            .to_request();
        let start = Instant::now();
        let resp = test::call_service(&app, req).await;
        let duration1 = start.elapsed().as_nanos();
        assert_eq!(resp.status(), StatusCode::OK);
        // 2nd time with correct auth info, to check cache is working
        let req = test::TestRequest::default()
            .append_header((
                "Authorization",
                format!("Basic {}", base64::STANDARD.encode("myuser:mypassword")),
            ))
            .to_request();
        let start = Instant::now();
        let resp = test::call_service(&app, req).await;
        let duration2 = start.elapsed().as_nanos();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(duration2 < duration1 / 10);
    }
}
