use anyhow::Result;

#[actix_web::main]
async fn main() -> Result<()> {
    let cli = my_cont_srv::config::parse_cli();

    if let Some(hash_password) = cli.hash_password {
        let hash = bcrypt::hash(hash_password, bcrypt::DEFAULT_COST)?;
        println!("{}", hash);
        return Ok(());
    }

    let config = my_cont_srv::config::get_config(cli)?;
    let server = my_cont_srv::create_server(config).await?;
    server.await?;
    Ok(())
}
