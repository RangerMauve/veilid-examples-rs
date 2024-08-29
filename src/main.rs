use anyhow::{anyhow, Ok, Result};
use std::{env, sync::Arc};
use tokio::sync::mpsc::{self, Receiver};
use veilid_core::{
    api_startup_config, DHTSchema, UpdateCallback, VeilidAPI, VeilidConfigBlockStore,
    VeilidConfigInner, VeilidConfigProtectedStore, VeilidConfigTableStore, VeilidUpdate,
    CRYPTO_KIND_VLD0,
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

    let args: Vec<String> = env::args().collect();

    if args.len() > 0 {
        let command = &args[1];
        println!("Running command {:?}", command);

        if command.eq("dht-get-set") {
            dht_set_get(&veilid).await.unwrap();
        }
    }

    println!("Dropping update processor task");

    veilid.shutdown().await;

    handle.abort();
}

async fn dht_set_get(veilid: &VeilidAPI) -> Result<()> {
    let schema = DHTSchema::dflt(1)?;
    let routing_context = veilid.routing_context()?;
    let record = routing_context
        .create_dht_record(schema, Some(CRYPTO_KIND_VLD0))
        .await?;

    let key = record.key();
    let set_value = "Hello World";

    println!("Setting value {:?} at key {:?}", set_value, key);

    routing_context
        .set_dht_value(*key, 0, set_value.as_bytes().to_vec(), None)
        .await?;

    println!("reading value back");

    // Close the record when you're done with it
    routing_context.close_dht_record(*key).await?;

    // Make sure to re-open before reading again
    let record = routing_context.open_dht_record(*key, None).await?;

    let get_value = routing_context
        .get_dht_value(*key, 0, true)
        .await?
        .unwrap()
        .data()
        .to_vec();

    println!("Got value {:?}", String::from_utf8(get_value)?);
    return Ok(());
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
