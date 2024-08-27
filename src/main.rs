use anyhow::{anyhow, Result};
use std::sync::Arc;
use tokio::sync::mpsc::{self, Receiver};
use veilid_core::{
    api_startup_config, UpdateCallback, VeilidAPI, VeilidConfigBlockStore, VeilidConfigInner,
    VeilidConfigProtectedStore, VeilidConfigTableStore, VeilidUpdate,
};

#[tokio::main]
async fn main() {
    println!("Hello, world!");

    let (veilid, mut rx) = init_veilid().await.unwrap();

    let handle = tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            println!("Got update!")
        }
    });

    handle.abort();

    veilid.shutdown().await;
}

async fn init_veilid() -> Result<(VeilidAPI, Receiver<VeilidUpdate>)> {
    let (tx, mut rx) = mpsc::channel(32);

    let update_callback: UpdateCallback = Arc::new(move |update| {
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(_) = tx.send(update).await {
                println!("receiver dropped");
                return;
            }
        });
    });

    let config: VeilidConfigInner = VeilidConfigInner {
        program_name: "Veilid Examples RS".into(),
        namespace: "veilid-examples-rs".into(),
        protected_store: VeilidConfigProtectedStore {
            // avoid prompting for password, don't do this in production
            always_use_insecure_storage: true,
            directory: "./.veilid/block_store".into(),
            ..Default::default()
        },
        block_store: VeilidConfigBlockStore {
            directory: "./.veilid/block_store".into(),
            ..Default::default()
        },
        table_store: VeilidConfigTableStore {
            directory: "./.veilid/table_store".into(),
            ..Default::default()
        },
        ..Default::default()
    };

    let veilid = api_startup_config(update_callback, config)
        .await
        .map_err(|e| anyhow!("Failed to initialize Veilid API: {}", e))?;

    println!("Attaching Veilid node");
    // Make sure to attach before doing stuff
    veilid.attach().await?;

    println!("Waiting for internet cxonnection");
    // DO this in your app before trying to init routing contexts
    wait_for_network(&mut rx).await?;

    return Ok((veilid, rx));
}

async fn wait_for_network(rx: &mut Receiver<VeilidUpdate>) -> Result<()> {
    while let Some(update) = rx.recv().await {
        if let VeilidUpdate::Attachment(attachment_state) = update {
            if attachment_state.public_internet_ready {
                println!("Public internet ready!");
                break;
            }
        }
    }
    return Ok(());
}
