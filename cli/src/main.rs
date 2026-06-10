use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use shared::dbus::{
    value_as_bool, VariantMap, DAEMON_INTERFACE, DEVICES_INTERFACE, EFFECTS_INTERFACE, OBJECT_PATH,
    SERVICE_NAME,
};
use zbus::{Connection, Proxy};
use zvariant::{OwnedValue, Str, Value};

#[derive(Debug, Parser)]
#[command(name = "openeffectsctl", about = "Control the OpenEffects daemon")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Status {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        short: bool,
    },
    Start,
    Stop,
    Enable {
        effect: String,
    },
    Disable {
        effect: String,
    },
    Toggle {
        effect: String,
    },
    Set {
        assignment: String,
        value: String,
    },
    Camera {
        #[command(subcommand)]
        command: CameraCommand,
    },
    Watch,
}

#[derive(Debug, Subcommand)]
enum CameraCommand {
    List,
    Select { id: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let conn = Connection::session().await?;

    match cli.command {
        Command::Status { json, short } => status(&conn, json, short).await?,
        Command::Start => {
            daemon_proxy(&conn)
                .await?
                .call::<_, _, ()>("Start", &())
                .await?
        }
        Command::Stop => {
            daemon_proxy(&conn)
                .await?
                .call::<_, _, ()>("Stop", &())
                .await?
        }
        Command::Enable { effect } => {
            effects_proxy(&conn)
                .await?
                .call::<_, _, ()>("SetEnabled", &(effect.as_str(), true))
                .await?
        }
        Command::Disable { effect } => {
            effects_proxy(&conn)
                .await?
                .call::<_, _, ()>("SetEnabled", &(effect.as_str(), false))
                .await?
        }
        Command::Toggle { effect } => toggle(&conn, &effect).await?,
        Command::Set { assignment, value } => set_param(&conn, &assignment, &value).await?,
        Command::Camera { command } => camera(&conn, command).await?,
        Command::Watch => watch(&conn).await?,
    }

    Ok(())
}

async fn status(conn: &Connection, json: bool, short: bool) -> anyhow::Result<()> {
    let daemon = daemon_proxy(conn).await?;
    let status = daemon.get_property::<String>("Status").await?;
    if short {
        println!("{status}");
        return Ok(());
    }
    let caps = daemon
        .get_property::<VariantMap>("Capabilities")
        .await
        .unwrap_or_default();
    let tier = simple_value(&caps, "tier").unwrap_or_else(|| "unknown".into());
    let ep = simple_value(&caps, "ep").unwrap_or_else(|| "unknown".into());
    let models = match simple_value(&caps, "models_ready").as_deref() {
        Some("true") => "ready",
        _ => "missing",
    };
    let sink = simple_value(&caps, "output_sink").unwrap_or_else(|| "unknown".into());
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": status,
                "tier": tier,
                "ep": ep,
                "models": models,
                "sink": sink,
            })
        );
    } else {
        println!("{status} (tier: {tier}, ep: {ep}, models: {models}, sink: {sink})");
    }
    Ok(())
}

async fn toggle(conn: &Connection, effect: &str) -> anyhow::Result<()> {
    let effects = effects_proxy(conn).await?;
    let params: VariantMap = effects.call("GetParams", &(effect)).await?;
    let next = !params
        .get("enabled")
        .and_then(value_as_bool)
        .unwrap_or(false);
    effects
        .call::<_, _, ()>("SetEnabled", &(effect, next))
        .await?;
    println!("{effect} {}", if next { "enabled" } else { "disabled" });
    Ok(())
}

async fn set_param(conn: &Connection, assignment: &str, raw_value: &str) -> anyhow::Result<()> {
    let (effect, key) = assignment
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("expected EFFECT.KEY, e.g. studio_light.brightness"))?;
    let value = parse_value(raw_value);
    let value: Value<'_> = value.into();
    effects_proxy(conn)
        .await?
        .call::<_, _, ()>("SetParam", &(effect, key, value))
        .await?;
    Ok(())
}

async fn camera(conn: &Connection, command: CameraCommand) -> anyhow::Result<()> {
    let devices = devices_proxy(conn).await?;
    match command {
        CameraCommand::List => {
            let cameras: Vec<VariantMap> = devices.call("ListCameras", &()).await?;
            for camera in cameras {
                let id = simple_value(&camera, "id").unwrap_or_default();
                let name = simple_value(&camera, "name").unwrap_or_else(|| id.clone());
                println!("{id}\t{name}");
            }
        }
        CameraCommand::Select { id } => {
            devices
                .call::<_, _, ()>("SelectCamera", &(id.as_str()))
                .await?
        }
    }
    Ok(())
}

async fn watch(conn: &Connection) -> anyhow::Result<()> {
    let proxy = Proxy::new(conn, SERVICE_NAME, OBJECT_PATH, EFFECTS_INTERFACE).await?;
    let mut stream = proxy.receive_signal("EffectChanged").await?;
    while let Some(message) = stream.next().await {
        let body = message.body();
        let (id, params): (String, VariantMap) = body.deserialize()?;
        println!("{id}\t{}", format_params(&params));
    }
    Ok(())
}

fn parse_value(raw: &str) -> OwnedValue {
    if raw.eq_ignore_ascii_case("true") {
        return OwnedValue::from(true);
    }
    if raw.eq_ignore_ascii_case("false") {
        return OwnedValue::from(false);
    }
    if let Ok(value) = raw.parse::<u32>() {
        return OwnedValue::from(value);
    }
    if let Ok(value) = raw.parse::<i32>() {
        return OwnedValue::from(value);
    }
    OwnedValue::from(Str::from(raw.to_owned()))
}

fn simple_value(map: &VariantMap, key: &str) -> Option<String> {
    map.get(key)
        .and_then(shared::dbus::value_as_string)
        .or_else(|| {
            map.get(key)
                .and_then(shared::dbus::value_as_u32)
                .map(|v| v.to_string())
        })
        .or_else(|| {
            map.get(key)
                .and_then(shared::dbus::value_as_i32)
                .map(|v| v.to_string())
        })
        .or_else(|| {
            map.get(key)
                .and_then(shared::dbus::value_as_bool)
                .map(|v| v.to_string())
        })
}

fn format_params(params: &VariantMap) -> String {
    let mut parts = params
        .iter()
        .map(|(key, value)| {
            let value = simple_value(params, key).unwrap_or_else(|| format!("{value:?}"));
            format!("{key}={value}")
        })
        .collect::<Vec<_>>();
    parts.sort();
    parts.join(" ")
}

async fn daemon_proxy(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(conn, SERVICE_NAME, OBJECT_PATH, DAEMON_INTERFACE).await
}

async fn effects_proxy(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(conn, SERVICE_NAME, OBJECT_PATH, EFFECTS_INTERFACE).await
}

async fn devices_proxy(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(conn, SERVICE_NAME, OBJECT_PATH, DEVICES_INTERFACE).await
}
