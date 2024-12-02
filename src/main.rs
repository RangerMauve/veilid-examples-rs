use anyhow::{anyhow, Ok, Result};
use core::str;
use std::{env, sync::Arc};
use tokio::{
    sync::mpsc::{self, Receiver},
    task::JoinHandle,
};
use veilid_core::{
    api_startup_config, DHTSchema, RouteId, UpdateCallback, VeilidAPI, VeilidConfigBlockStore,
    VeilidConfigInner, VeilidConfigProtectedStore, VeilidConfigTableStore, VeilidUpdate,
    CRYPTO_KIND_VLD0, VALID_CRYPTO_KINDS,
};

#[tokio::main]
async fn main() {
    println!("Hello, world!");

    let (veilid, rx) = init_veilid().await.unwrap();

    let mut handle: Option<JoinHandle<()>> = None;

    let args: Vec<String> = env::args().collect();

    if args.len() > 1 {
        let command = &args[1];
        println!("Running command {:?}", command);

        if command.eq("dht-get-set") {
            handle = Some(log_updates(rx));

            dht_set_get(&veilid).await.unwrap();
        } else if command.eq("private-route-app-message") {
            private_route_app_message(&veilid, rx).await.unwrap();
        } else if command.eq("private-route-app-call") {
            private_route_app_call(&veilid, rx).await.unwrap();
        }
    } else {
        handle = Some(log_updates(rx));
    }

    println!("Dropping update processor task");

    veilid.shutdown().await;

    if handle.is_some() {
        handle.unwrap().abort();
    }

    println!("Goodbye!");
}

async fn private_route_app_message(
    veilid: &VeilidAPI,
    mut rx: Receiver<VeilidUpdate>,
) -> Result<()> {
    let (tx_got_message, mut rx_got_message) = mpsc::channel(1);
    // This wrapper has retry logic
    let (route_id, route_id_blob) = make_route(veilid).await?;

    // Listen for app messages for your route elsewhere
    let handle = tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            if let VeilidUpdate::AppMessage(app_message) = update {
                println!("got app message in manager");
                if !app_message.route_id().is_some() {
                    println!("app message without route");
                }
                let got_route_id = app_message.route_id().unwrap().to_owned();
                if got_route_id != route_id {
                    println!("app message for other route {:?}", got_route_id);
                }
                tx_got_message.send(app_message).await.unwrap();
            }
        }
    });

    // You only need the blob to import the route, store it on the DHT or share via link
    // Make sure to import before sending app messages
    let route_id = veilid.import_remote_private_route(route_id_blob)?;

    veilid
        .routing_context()?
        .app_message(
            veilid_core::Target::PrivateRoute(route_id),
            "Hello World!".as_bytes().to_vec(),
        )
        .await?;

    let app_message = rx_got_message.recv().await.unwrap();

    let message = str::from_utf8(app_message.message())?;
    println!("AppMessage {:?}", message);

    handle.abort();

    return Ok(());
}

async fn private_route_app_call(veilid: &VeilidAPI, mut rx: Receiver<VeilidUpdate>) -> Result<()> {
    let (tx_got_message, mut rx_got_message) = mpsc::channel(1);
    // This wrapper has retry logic
    let (route_id, route_id_blob) = make_route(veilid).await?;

    let veilid_responder = veilid.clone();
    // Listen for app messages for your route elsewhere
    let handle = tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            if let VeilidUpdate::AppCall(app_call) = update {
                println!("got app call in manager");
                if !app_call.route_id().is_some() {
                    println!("app call without route");
                }
                let got_route_id = app_call.route_id().unwrap().to_owned();
                if got_route_id != route_id {
                    println!("app call for other route {:?}", got_route_id);
                }
                let call_id = app_call.id();
                tx_got_message.send(app_call).await.unwrap();
                veilid_responder
                    .app_call_reply(call_id, "Hey".as_bytes().to_vec())
                    .await
                    .unwrap();
            }
        }
    });

    // You only need the blob to import the route, store it on the DHT or share via link
    // Make sure to import before sending app messages
    let route_id = veilid.import_remote_private_route(route_id_blob)?;

    let response = veilid
        .routing_context()?
        .app_call(
            veilid_core::Target::PrivateRoute(route_id),
            "Hello World!".as_bytes().to_vec(),
        )
        .await?;

    let app_call = rx_got_message.recv().await.unwrap();

    let message = str::from_utf8(app_call.message())?;
    println!("AppCall {:?}", message);
    println!("Response {:?}", str::from_utf8(response.as_slice())?);

    handle.abort();

    return Ok(());
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
    let _record = routing_context.open_dht_record(*key, None).await?;

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

fn log_updates(mut rx: Receiver<VeilidUpdate>) -> JoinHandle<()> {
    return tokio::spawn(async move {
        while let Some(_update) = rx.recv().await {
            println!("Got update!")
        }
    });
}

async fn make_route(veilid: &VeilidAPI) -> Result<(RouteId, Vec<u8>)> {
    let mut retries = 3;
    while retries != 0 {
        retries -= 1;
        let result = veilid
            .new_custom_private_route(
                &VALID_CRYPTO_KINDS,
                veilid_core::Stability::LowLatency,
                veilid_core::Sequencing::NoPreference,
            )
            .await;

        if result.is_ok() {
            return Ok(result.unwrap());
        }
    }
    return Err(anyhow!("Unable to create route, reached max retries"));
}
