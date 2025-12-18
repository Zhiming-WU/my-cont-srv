use crate::AppState;
use actix_web::{HttpRequest, HttpResponse, web};
use std::path::PathBuf;
use tokio::fs;
use tokio_util::io::ReaderStream;

fn format_size(size: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut size = size as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}

async fn dir_get(req: &HttpRequest, path: &PathBuf) -> HttpResponse {
    let mut out = String::from("");
    let dir = fs::read_dir(&path).await;
    if dir.is_err() {
        return HttpResponse::InternalServerError().body(format!(
            "Reading dir[{}] failed: {:?}",
            path.to_string_lossy(),
            dir.err().unwrap()
        ));
    }
    let mut dir = dir.unwrap();
    let mut vec = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await {
        vec.push(entry);
    }
    vec.sort_unstable_by_key(|a| a.file_name());

    for entry in vec {
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        out.push_str("<div>");
        if file_type.is_dir() {
            out.push_str("[+&nbsp;");
        } else {
            out.push_str("[-&nbsp;");
        }
        let mut url = String::from(req.path());
        if !url.ends_with("/") {
            url.push('/');
        }
        url.push_str(&urlencoding::encode(name));
        let anchor = format!(r#"<a href="{}">{}</a>]"#, &url, &name);
        out.push_str(&anchor);
        if file_type.is_file()
            && let Ok(meta) = entry.metadata().await
        {
            out.push_str(&format!("&nbsp;[{}]", format_size(meta.len()),));
        }
        if name.ends_with(".epub") {
            out.push_str(&format!(r#"&nbsp;[<a href="/epub_toc{}">Read</a>]"#, &url));
        }
        out.push_str("</div>");
    }

    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(out)
}

async fn file_get(size: u64, path: &PathBuf) -> HttpResponse {
    let file = fs::File::open(path).await;
    if file.is_err() {
        return HttpResponse::InternalServerError().body(format!(
            "Opening file [{:?}] failed: {:?}",
            &path,
            file.err().unwrap()
        ));
    }
    let mut resp_builder = HttpResponse::Ok();
    resp_builder.insert_header(("Content-Length", size.to_string()));
    if let Some(mime) = mime_guess::from_path(path).first() {
        resp_builder.content_type(mime);
    }
    resp_builder.streaming(ReaderStream::new(file.unwrap()))
}

pub async fn fs_get(req: HttpRequest, app_state: web::Data<AppState>) -> HttpResponse {
    let mut path = app_state.root_dir.clone();
    let Ok(decoded_path) = urlencoding::decode(req.path()) else {
        return HttpResponse::BadRequest().body("Invalid request path");
    };
    if req.path() != "/" {
        path.push(&(&*decoded_path)[1..]);
    }
    let Ok(meta) = fs::metadata(&path).await else {
        return HttpResponse::NotFound().body("Resource not found");
    };

    if meta.is_dir() {
        return dir_get(&req, &path).await;
    }

    if meta.is_file() {
        return file_get(meta.len(), &path).await;
    }

    HttpResponse::NotFound().body("Resource not found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use actix_http::StatusCode;
    use actix_web::dev::ServiceResponse;
    use actix_web::test;

    #[actix_web::test]
    async fn test_fs_get_default_root() {
        let req = test::TestRequest::default().to_http_request();
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let resp = fs_get(req.clone(), app_data).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(ServiceResponse::new(req, resp)).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"[-&nbsp;<a href="/Cargo.toml">Cargo.toml</a>]"#));
        assert!(body.contains(r#"[+&nbsp;<a href="/src">src</a>]"#));
    }

    #[actix_web::test]
    async fn test_fs_get_other_root() {
        let req = test::TestRequest::default().to_http_request();
        let app_data = web::Data::new(AppState::new(PathBuf::from("src")));
        let resp = fs_get(req.clone(), app_data).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(ServiceResponse::new(req, resp)).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"[-&nbsp;<a href="/main.rs">main.rs</a>]"#));
    }

    #[actix_web::test]
    async fn test_fs_get_dir_with_uri() {
        let req = test::TestRequest::default().uri("/src").to_http_request();
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let resp = fs_get(req.clone(), app_data).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(ServiceResponse::new(req, resp)).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"[-&nbsp;<a href="/src/main.rs">main.rs</a>]"#));
    }

    #[actix_web::test]
    async fn test_fs_get_dir_with_epub() {
        let req = test::TestRequest::default()
            .uri("/res_dir")
            .to_http_request();
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let resp = fs_get(req.clone(), app_data).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(ServiceResponse::new(req, resp)).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"[65.92 KB]&nbsp;[<a href="/epub_toc/res_dir/v2.epub">Read</a>]"#));
    }

    #[actix_web::test]
    async fn test_fs_get_pdf_file() {
        let req = test::TestRequest::default()
            .uri("/res_dir/dummy.pdf")
            .to_http_request();
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let resp = fs_get(req.clone(), app_data).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().contains_key("Content-Type"));
        assert_eq!(
            resp.headers().get("Content-Type").unwrap(),
            "application/pdf"
        );
        let body = test::read_body(ServiceResponse::new(req, resp)).await;
        let file_cont = fs::read("res_dir/dummy.pdf").await.unwrap();
        assert!(body.to_vec() == file_cont);
    }

    #[actix_web::test]
    async fn test_fs_get_non_exist() {
        let req = test::TestRequest::default()
            .uri("/non_exist")
            .to_http_request();
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let resp = fs_get(req.clone(), app_data).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
