use crate::types::Trade;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

pub async fn run(mut rx: mpsc::Receiver<Trade>) -> anyhow::Result<()> {
    let mut stdout = tokio::io::stdout();

    while let Some(trade) = rx.recv().await {
        let line = serde_json::to_string(&trade)?;
        stdout.write_all(line.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        // No flush per-line — stdout is line-buffered when attached to a terminal
        // and fully buffered when piped, which is fine for high-volume output.
    }
    Ok(())
}
