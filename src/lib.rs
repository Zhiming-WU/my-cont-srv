use actix_web::middleware::{Compress, Condition};
use actix_web::{App, HttpServer, dev::ServiceRequest, web};
use actix_web_httpauth::{
    extractors::{AuthenticationError, basic::BasicAuth},
    middleware::HttpAuthentication,
};
use anyhow::Result;
use lru::LruCache;
use rustls::ServerConfig;
use std::cell::RefCell;
use std::io::BufReader;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub mod config;
mod epub_proc;
mod fs_proc;

fn app_config(cfg: &mut web::ServiceConfig) {
    cfg.route(
        "/epub_toc/{filepath:.*}",
        web::get().to(epub_proc::epub_toc),
    );
    cfg.route(
        "/epub_cont/{filepath}/{innerpath:.*}",
        web::get().to(epub_proc::epub_cont),
    );
    cfg.default_service(web::get().to(fs_proc::fs_get));
}

struct AppState {
    root_dir: PathBuf,
    epub_toc_cache: Mutex<LruCache<String, String>>,
    epub_cont_cache: Mutex<LruCache<String, (String, Vec<u8>)>>,
}

impl AppState {
    fn new(root_dir: PathBuf) -> Self {
        AppState {
            root_dir,
            epub_toc_cache: Mutex::new(LruCache::new(NonZeroUsize::new(10).unwrap())),
            epub_cont_cache: Mutex::new(LruCache::new(NonZeroUsize::new(200).unwrap())),
        }
    }
}

fn tls_config(cert_path: &PathBuf, key_path: &PathBuf) -> Result<ServerConfig> {
    let builder = ServerConfig::builder().with_no_client_auth();

    let cert_file = &mut BufReader::new(std::fs::File::open(cert_path)?);
    let key_file = &mut BufReader::new(std::fs::File::open(key_path)?);

    let cert_chain = rustls_pemfile::certs(cert_file)
        .map(|item| item.unwrap())
        .collect();
    let key = rustls_pemfile::private_key(key_file)?;

    let config = builder.with_single_cert(cert_chain, key.unwrap())?;
    Ok(config)
}

#[derive(Clone)]
struct AuthInfo {
    user: String,
    hash: String,
    cached_pass: Arc<Mutex<RefCell<String>>>,
}

impl AuthInfo {
    fn new(user: &str, hash: &str) -> Self {
        Self {
            user: String::from(user),
            hash: String::from(hash),
            cached_pass: Arc::new(Mutex::new(RefCell::new(String::new()))),
        }
    }
}

async fn basic_auth(
    req: ServiceRequest,
    cred: BasicAuth,
) -> Result<ServiceRequest, (actix_web::error::Error, ServiceRequest)> {
    let info = req.app_data::<AuthInfo>().unwrap();
    let mut failed = false;
    if cred.user_id() != &info.user {
        failed = true;
    }
    if !failed {
        if cred.password().is_none() {
            failed = true;
        } else {
            let cached: bool;
            let provided = cred.password().unwrap();
            {
                let cached_pass = info.cached_pass.lock().await;
                let cached_pass = cached_pass.borrow();
                cached = !cached_pass.is_empty();
                failed = cached && cached_pass.as_str() != provided;
            }
            if !cached {
                let res = bcrypt::verify(provided, &info.hash);
                failed = !(res.is_ok() && res.unwrap());
                if !failed {
                    let cached_pass = info.cached_pass.lock().await;
                    cached_pass.borrow_mut().push_str(provided);
                }
            }
        }
    }
    if !failed {
        return Ok(req);
    }
    let config =
        actix_web_httpauth::extractors::basic::Config::default().realm("My-Content-Server");
    Err((AuthenticationError::from(config).into(), req))
}

pub async fn create_server(config: config::Config) -> Result<actix_server::Server> {
    let mut auth_info = AuthInfo::new("", "");

    let enable_auth = config.user_name.is_some() && config.password_hash.is_some();
    if enable_auth {
        auth_info.user = config.user_name.unwrap();
        auth_info.hash = config.password_hash.unwrap();
    }

    let app_data = web::Data::new(AppState::new(config.root_dir));
    let app = move || {
        let mut app = App::new().configure(app_config).app_data(app_data.clone());
        if enable_auth {
            app = app.app_data(auth_info.clone());
        }
        app.wrap(Compress::default()).wrap(Condition::new(
            enable_auth,
            HttpAuthentication::basic(basic_auth),
        ))
    };

    let addrs = format!("{}:{}", config.address, config.port);
    let mut server = HttpServer::new(app).workers(config.workers);
    if config.cert_path.is_some() && config.key_path.is_some() {
        server = server.bind_rustls_0_23(
            addrs,
            tls_config(&config.cert_path.unwrap(), &config.key_path.unwrap())?,
        )?;
    } else {
        server = server.bind(addrs)?;
    }
    let result = server.run();

    Ok(result)
}
