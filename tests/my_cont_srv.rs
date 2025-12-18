use my_cont_srv::{
    config::{get_config, parse_cli_from},
    create_server,
};

fn args_to_vec(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn test_https() {
    let cli = parse_cli_from(args_to_vec(&["my-cont-srv", "-c", "res_dir/config.toml"]));
    let cfg = get_config(cli).unwrap();
    let server = create_server(cfg).await.unwrap();
    let server_handle = server.handle();
    tokio::spawn(server);

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let resp = client
        .get("https://127.0.0.1:11310")
        .basic_auth("myuser", Some("mypassword"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    server_handle.stop(true).await;
}
