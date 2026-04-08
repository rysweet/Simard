mod auth;
mod routes;

use std::net::SocketAddr;

pub fn serve(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let code = auth::init_login_code();
    eprintln!("\n  🌲 Simard Dashboard");
    eprintln!("  Login code: {code}");
    eprintln!("  Open http://localhost:{port} and enter the code\n");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let app = routes::build_router();

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        eprintln!("Simard dashboard listening on http://{addr}");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}
