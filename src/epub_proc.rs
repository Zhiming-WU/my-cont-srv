use crate::AppState;
use ::base64::Engine;
use actix_web::{HttpResponse, Responder, web};
use base64::engine::general_purpose as base64;
use epub::doc::{EpubDoc, NavPoint};
use std::sync::LazyLock;
use std::{
    io::{Read, Seek},
    path::PathBuf,
};

fn resp_navpoint(out: &mut String, level: u8, nav: &NavPoint) {
    out.push_str("<div>");
    for _ in 0..level {
        out.push_str("&emsp;");
    }
    out.push_str(&format!(r#"<a href="{}">"#, nav.content.to_string_lossy()));
    out.push_str(&nav.label);
    out.push_str("</a></div>");
    for child in &nav.children {
        resp_navpoint(out, level + 1, child);
    }
}

pub async fn epub_toc(req_path: web::Path<String>, app_state: web::Data<AppState>) -> HttpResponse {
    let path = req_path.into_inner();

    let mut out = String::new();
    let mut cached = false;
    {
        let mut cache = app_state.epub_toc_cache.lock().await;
        if cache.contains(&path) {
            out = cache.get(&path).unwrap().to_owned();
            cached = true;
        }
    }
    if cached {
        return HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(out);
    }

    let mut file_path = app_state.root_dir.clone();
    file_path.push(&path);
    let doc = EpubDoc::new(&file_path);
    if doc.is_err() {
        return HttpResponse::InternalServerError()
            .content_type("text/html; charset=utf-8")
            .body(format!(
                "Reading/Parsing epub [{:?}] failed: {:?}",
                &path,
                doc.err().unwrap()
            ));
    }
    let doc = doc.unwrap();

    let b64_path = base64::URL_SAFE_NO_PAD.encode(&path);

    if doc.toc.len() == 0 {
        if doc.spine.len() > 0
            && let Some(res_item) = doc.resources.get(&doc.spine[0].idref)
        {
            return epub_cont_proc(
                b64_path,
                res_item.path.to_string_lossy().to_string(),
                app_state,
            )
            .await;
        }
        return HttpResponse::NotFound().body("No contents found in the epub file");
    }

    out.push_str(&format!(
        r#"<head><base href="/epub_cont/{}/"/></head>"#,
        b64_path
    ));

    out.push_str("<body>");
    for item in &doc.toc {
        resp_navpoint(&mut out, 0, item);
    }
    out.push_str("</body>");

    {
        let mut cache = app_state.epub_toc_cache.lock().await;
        cache.put(path, out.clone());
    }

    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(out)
}

#[inline]
fn resp_epub_cont(mine: String, cont: Vec<u8>) -> HttpResponse {
    if !mine.is_empty() {
        return HttpResponse::Ok().content_type(mine).body(cont);
    }
    HttpResponse::Ok().body(cont)
}

fn epub_gen_html_nav_elem<R: Read + Seek>(
    doc: &EpubDoc<R>,
    b64_path: &str,
    file_path: &str,
    inner_path: &str,
) -> Option<String> {
    let inner_path = PathBuf::from(inner_path);
    for (id, item) in doc.resources.iter() {
        if item.path == inner_path {
            let mut has_nav = false;
            let idx = doc.spine.iter().position(|item| &item.idref == id)?;
            let mut out = String::from(
                r#"<div style="display: flex; justify-content: space-between; align-items: center;">"#,
            );
            if idx > 0
                && let Some(prev_item) = doc.resources.get(&doc.spine[idx - 1].idref)
            {
                out.push_str(&format!(
                    //r#"<a style="float: left" href="/epub_cont/{}/{}">Prev</a>"#,
                    r#"<a href="/epub_cont/{}/{}">Prev</a>"#,
                    b64_path,
                    prev_item.path.to_string_lossy()
                ));
                has_nav = true;
            } else {
                out.push_str(r#"<span style="color:grey">Prex</span>"#);
            }
            if doc.toc.len() > 0 {
                out.push_str(&format!(
                    r#"<a href="/epub_toc/{}">Table of Contents</a>"#,
                    urlencoding::encode(file_path)
                ));
                has_nav = true;
            } else {
                out.push_str(r#"<span style="color:grey">Table of Contents</span>"#);
            }
            if idx < doc.spine.len() - 1
                && let Some(next_item) = doc.resources.get(&doc.spine[idx + 1].idref)
            {
                out.push_str(&format!(
                    r#"<a href="/epub_cont/{}/{}">Next</a>"#,
                    b64_path,
                    next_item.path.to_string_lossy()
                ));
                has_nav = true;
            } else {
                out.push_str(r#"<span style="color:grey">Next</span>"#);
            }
            if !has_nav {
                return None;
            }
            out.push_str("</div>");
            return Some(out);
        }
    }
    None
}

async fn epub_cont_proc(
    file_path: String,
    inner_path: String,
    app_state: web::Data<AppState>,
) -> HttpResponse {
    let whole_path = format!("{}/{}", file_path, inner_path);
    let (mut mime, mut cont) = (String::new(), Vec::<u8>::new());

    let mut cached = false;
    {
        let mut cache = app_state.epub_cont_cache.lock().await;
        if cache.contains(&whole_path) {
            (mime, cont) = cache.get(&whole_path).unwrap().to_owned();
            cached = true;
        }
    }
    if cached {
        return resp_epub_cont(mime, cont);
    }

    let path = base64::URL_SAFE_NO_PAD.decode(&file_path);
    if path.is_err() {
        return HttpResponse::BadRequest().body(format!(
            "Invalid file path [{}]: base64 decoding failed: {:?}",
            &file_path,
            path.err().unwrap()
        ));
    }
    let path = path.unwrap();
    let path_str = String::from_utf8_lossy(&path);

    let mut path_buf = app_state.root_dir.clone();
    path_buf.push(&*path_str);
    let doc = EpubDoc::new(&path_buf);
    if doc.is_err() {
        return HttpResponse::InternalServerError()
            .content_type("text/html; charset=utf-8")
            .body(format!(
                "Reading/Parsing epub [{:?}] failed: {:?}",
                &path,
                doc.err().unwrap()
            ));
    }
    let mut doc = doc.unwrap();

    let cont_res = doc.get_resource_by_path(&inner_path);
    if cont_res.is_none() {
        return HttpResponse::NotFound().body(format!("Resource [{}] not found", inner_path));
    }
    let mut cont = cont_res.unwrap();
    if let Some(mime_str) = doc.get_resource_mime_by_path(&inner_path) {
        mime = mime_str;
    }
    if mime.is_empty() && inner_path.contains("htm") {
        mime = String::from("text/html; charset=utf-8");
    }

    if mime.contains("htm") {
        if let Some(nav) = epub_gen_html_nav_elem(&doc, &file_path, &*path_str, &inner_path) {
            static RE: LazyLock<regex::Regex> =
                LazyLock::new(|| regex::Regex::new("<body.*?>").unwrap());
            let cont_str = String::from_utf8_lossy(&cont);
            cont = RE
                .replace_all(&cont_str, &format!(r#"$0{}"#, &nav))
                .replace("</body>", &format!("{}</body>", &nav))
                .as_bytes()
                .to_vec();
        }
    }

    {
        let mut cache = app_state.epub_cont_cache.lock().await;
        cache.put(whole_path, (mime.clone(), cont.clone()));
    }

    resp_epub_cont(mime, cont)
}

pub async fn epub_cont(
    req_path: web::Path<(String, String)>,
    app_state: web::Data<AppState>,
) -> impl Responder {
    let (file_path, inner_path) = req_path.into_inner();
    epub_cont_proc(file_path, inner_path, app_state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppState, app_config};
    use actix_http::StatusCode;
    use actix_web::{App, test};
    use std::time::Instant;

    #[actix_web::test]
    async fn test_epub_toc_v2() {
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let app = test::init_service(App::new().configure(app_config).app_data(app_data)).await;
        let req = test::TestRequest::default()
            .uri("/epub_toc/res_dir/v2.epub")
            .to_request();
        let start = Instant::now();
        let resp = test::call_service(&app, req).await;
        let duration1 = start.elapsed().as_nanos();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"<base href="/epub_cont/cmVzX2Rpci92Mi5lcHVi/"/>"#));
        assert!(body.contains(r#"<div><a href="OEBPS/valentinhauy11.html#ops1">Valentin Haüy"#));
        assert!(body.contains(r#" The father of the education for the blind</a></div>"#));
        // to check whether cache is working
        let req = test::TestRequest::default()
            .uri("/epub_toc/res_dir/v2.epub")
            .to_request();
        let start = Instant::now();
        let resp = test::call_service(&app, req).await;
        let duration2 = start.elapsed().as_nanos();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(duration2 < duration1 / 10);
    }

    #[actix_web::test]
    async fn test_epub_toc_v3() {
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let app = test::init_service(App::new().configure(app_config).app_data(app_data)).await;
        let req = test::TestRequest::default()
            .uri("/epub_toc/res_dir/nav.epub")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"<base href="/epub_cont/cmVzX2Rpci9uYXYuZXB1Yg/"/>"#));
        assert!(body.contains(r#"<div><a href="EPUB/s04.xhtml#pgepubid00492">SECTION IV FAIRY "#));
        assert!(body.contains(r#"STORIES—MODERN FANTASTIC TALES</a></div>"#));
    }

    #[actix_web::test]
    async fn test_epub_toc_v3_no_toc() {
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let app = test::init_service(App::new().configure(app_config).app_data(app_data)).await;
        let req = test::TestRequest::default()
            .uri("/epub_toc/res_dir/v3.epub")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"<title>EPUB 3.0 Specification</title>"#));
        assert!(body.contains(r#"<body><div style="display: flex;"#));
        assert!(body.contains(r#"<span style="color:grey">Prex</span>"#));
        assert!(body.contains(r#"<span style="color:grey">Table of Contents</span>"#));
        assert!(body.contains(r#"epub30-nav.xhtml">Next</a></div>"#));
        assert!(body.contains(r#"Next</a></div></body>"#));
    }

    #[actix_web::test]
    async fn test_epub_cont_v2() {
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let app = test::init_service(App::new().configure(app_config).app_data(app_data)).await;
        let req = test::TestRequest::default()
            .uri("/epub_cont/cmVzX2Rpci92Mi5lcHVi/OEBPS/valentinhauy11.html")
            .to_request();
        let start = Instant::now();
        let resp = test::call_service(&app, req).await;
        let duration1 = start.elapsed().as_nanos();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"<title>Valentin Haüy"#));
        assert!(body.contains(r#"<body><div style="display: flex;"#));
        assert!(body.contains(r#"Table of Contents</a>"#));
        let req = test::TestRequest::default()
            .uri("/epub_cont/cmVzX2Rpci92Mi5lcHVi/OEBPS/valentinhauy11.html")
            .to_request();
        let start = Instant::now();
        let resp = test::call_service(&app, req).await;
        let duration2 = start.elapsed().as_nanos();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(duration2 < duration1 / 10);
    }

    #[actix_web::test]
    async fn test_epub_cont_v3_first() {
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let app = test::init_service(App::new().configure(app_config).app_data(app_data)).await;
        let req = test::TestRequest::default()
            .uri("/epub_cont/cmVzX2Rpci92My5lcHVi/EPUB/xhtml/epub30-titlepage.xhtml")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"<title>EPUB 3.0 Specification</title>"#));
        assert!(body.contains(r#"<body><div style="display: flex;"#));
        assert!(body.contains(r#"<span style="color:grey">Prex</span>"#));
        assert!(body.contains(r#"<span style="color:grey">Table of Contents</span>"#));
        assert!(body.contains(r#"epub30-nav.xhtml">Next</a></div>"#));
        assert!(body.contains(r#"Next</a></div></body>"#));
    }

    #[actix_web::test]
    async fn test_epub_cont_v3_middle() {
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let app = test::init_service(App::new().configure(app_config).app_data(app_data)).await;
        let req = test::TestRequest::default()
            .uri("/epub_cont/cmVzX2Rpci92My5lcHVi/EPUB/xhtml/epub30-nav.xhtml")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"<title>EPUB 3 Specifications - Table of Contents</title>"#));
        assert!(body.contains(r#"<body><div style="display: flex;"#));
        assert!(body.contains(r#"epub30-titlepage.xhtml">Prev</a>"#));
        assert!(body.contains(r#"<span style="color:grey">Table of Contents</span>"#));
        assert!(body.contains(r#"epub30-terminology.xhtml">Next</a></div>"#));
        assert!(body.contains(r#"Next</a></div></body>"#));
    }

    #[actix_web::test]
    async fn test_epub_cont_v3_last() {
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let app = test::init_service(App::new().configure(app_config).app_data(app_data)).await;
        let req = test::TestRequest::default()
            .uri("/epub_cont/cmVzX2Rpci92My5lcHVi/EPUB/xhtml/epub30-changes.xhtml")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"<title>EPUB 3 Changes from EPUB 2.0.1</title>"#));
        assert!(body.contains(r#"<body><div style="display: flex;"#));
        assert!(body.contains(r#"epub30-references.xhtml">Prev</a>"#));
        assert!(body.contains(r#"<span style="color:grey">Table of Contents</span>"#));
        assert!(body.contains(r#"<span style="color:grey">Next</span></div>"#));
        assert!(body.contains(r#"Next</span></div></body>"#));
    }

    #[actix_web::test]
    async fn test_epub_toc_non_exist() {
        let app_data = web::Data::new(AppState::new(PathBuf::from(".")));
        let app = test::init_service(App::new().configure(app_config).app_data(app_data)).await;
        let req = test::TestRequest::default()
            .uri("/epub_toc/non_exist")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
