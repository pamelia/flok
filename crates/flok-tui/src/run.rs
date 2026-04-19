use anyhow::Result;
use tokio::sync::mpsc;

use crate::{app, app_event, tui, types::TuiChannels};

pub async fn run_app(channels: TuiChannels) -> Result<()> {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (tx, rx) = mpsc::unbounded_channel::<app_event::AppEvent>();
            let mut tui = tui::Tui::new(tx.clone())?;
            let mut app = app::App::new(channels, tx, rx);
            app.run(&mut tui).await
        })
        .await
}
