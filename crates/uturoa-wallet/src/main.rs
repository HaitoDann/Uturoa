//! `uturoa-wallet` : lance le portefeuille et l'ouvre dans le navigateur.

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "uturoa-wallet", version, about = "Portefeuille Uturoa")]
struct Cli {
    /// Dossier du portefeuille (par défaut : ~/.uturoa-wallet).
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Port local (0 : choisi automatiquement).
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Ne pas ouvrir le navigateur.
    #[arg(long)]
    no_browser: bool,
}

fn default_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".uturoa-wallet")
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if result.is_err() {
        println!("Ouvrez cette adresse dans votre navigateur : {url}");
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = cli.dir.unwrap_or_else(default_dir);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", cli.port)).await?;
        let port = listener.local_addr()?.port();
        let token = uturoa_wallet::app::new_token();
        let app = uturoa_wallet::app::router(dir.clone(), token);
        let url = format!("http://127.0.0.1:{port}/");
        println!("Portefeuille Uturoa : {url}");
        println!("Dossier : {}", dir.display());
        println!("Fermez cette fenêtre pour quitter.");
        if !cli.no_browser {
            open_browser(&url);
        }
        axum::serve(listener, app).await?;
        anyhow::Ok(())
    })
}
