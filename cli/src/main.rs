use clap::{Parser, Subcommand, ValueEnum};
use futures_util::StreamExt;
use shared::dbus::{
    DAEMON_INTERFACE, DEVICES_INTERFACE, EFFECTS_INTERFACE, OBJECT_PATH, SERVICE_NAME, VariantMap,
    value_as_bool,
};
use zbus::{Connection, Proxy};
use zvariant::{OwnedValue, Str, Value};

#[derive(Debug, Parser)]
#[command(
    name = "openeffectsctl",
    about = "Control the OpenEffects daemon",
    long_about = "Control the OpenEffects daemon over D-Bus.\n\n\
        Effects have friendly subcommands (center-stage, portrait-blur, background,\n\
        studio-light, reactions). Run one with no flags to print its current state,\n\
        or pass flags to change settings. `set` is a low-level escape hatch.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show daemon status and capabilities.
    Status {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
        /// Print only the one-word status (Waybar-friendly).
        #[arg(long)]
        short: bool,
    },
    /// Arm the virtual camera (camera opens on demand when a consumer connects).
    Start,
    /// Tear down the pipeline and release the camera.
    Stop,

    /// List every effect with its current enabled state.
    Effects,

    /// Enable an effect.
    Enable {
        #[arg(value_enum)]
        effect: EffectArg,
    },
    /// Disable an effect.
    Disable {
        #[arg(value_enum)]
        effect: EffectArg,
    },
    /// Flip an effect on/off.
    Toggle {
        #[arg(value_enum)]
        effect: EffectArg,
    },

    /// Center Stage: auto crop+zoom that keeps you framed.
    CenterStage {
        #[command(flatten)]
        toggle: EnableFlags,
        /// Zoom level.
        #[arg(long, value_enum)]
        zoom: Option<Zoom>,
        /// Framing mode.
        #[arg(long, value_enum)]
        mode: Option<Mode>,
        /// Manual crop inset from the top edge (0-512 px).
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=512))]
        crop_top: Option<u32>,
        /// Manual crop inset from the bottom edge (0-512 px).
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=512))]
        crop_bottom: Option<u32>,
        /// Manual crop inset from the left edge (0-512 px).
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=512))]
        crop_left: Option<u32>,
        /// Manual crop inset from the right edge (0-512 px).
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=512))]
        crop_right: Option<u32>,
    },

    /// Portrait Blur: blur the background behind you.
    PortraitBlur {
        #[command(flatten)]
        toggle: EnableFlags,
        /// Blur strength (0-100).
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
        strength: Option<u8>,
    },

    /// Background replace: swap the background for a color or image.
    Background {
        /// `none` to clear/disable, a `#RRGGBB` color, or a path to an image.
        /// Omit to print the current background.
        value: Option<String>,
        /// Disable the effect without changing the stored background.
        #[arg(long)]
        off: bool,
    },

    /// Studio Light: brighten and add contrast to your face.
    StudioLight {
        #[command(flatten)]
        toggle: EnableFlags,
        /// Overall intensity (0-100).
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
        intensity: Option<u8>,
        /// Brightness lift (-100 to 100).
        #[arg(long, value_parser = clap::value_parser!(i8).range(-100..=100))]
        brightness: Option<i8>,
        /// Contrast (0-100).
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
        contrast: Option<u8>,
    },

    /// Reactions: gesture/emoji overlay. Toggle the effect or fire a one-shot burst.
    Reactions {
        #[command(flatten)]
        toggle: EnableFlags,
    },
    /// Fire a one-shot emoji burst.
    React {
        #[arg(value_enum)]
        reaction: Reaction,
    },

    /// Low-level: set any effect param directly (EFFECT.KEY VALUE).
    Set {
        /// e.g. studio_light.brightness
        assignment: String,
        value: String,
    },

    /// Camera selection and virtual-camera info.
    Camera {
        #[command(subcommand)]
        command: CameraCommand,
    },

    /// Manage starting the daemon automatically with the desktop session.
    Autostart {
        #[command(subcommand)]
        command: AutostartCommand,
    },

    /// Stream EffectChanged signals until interrupted.
    Watch,
}

/// Shared --on / --off pair for effect subcommands.
#[derive(Debug, clap::Args)]
struct EnableFlags {
    /// Enable the effect.
    #[arg(long)]
    on: bool,
    /// Disable the effect.
    #[arg(long, conflicts_with = "on")]
    off: bool,
}

impl EnableFlags {
    fn resolve(&self) -> Option<bool> {
        if self.on {
            Some(true)
        } else if self.off {
            Some(false)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EffectArg {
    CenterStage,
    PortraitBlur,
    BgReplace,
    StudioLight,
    Reactions,
}

impl EffectArg {
    fn id(self) -> &'static str {
        match self {
            Self::CenterStage => "center_stage",
            Self::PortraitBlur => "portrait_blur",
            Self::BgReplace => "bg_replace",
            Self::StudioLight => "studio_light",
            Self::Reactions => "reactions",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Zoom {
    Subtle,
    Normal,
    Tight,
}

impl Zoom {
    fn id(self) -> &'static str {
        match self {
            Self::Subtle => "subtle",
            Self::Normal => "normal",
            Self::Tight => "tight",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Mode {
    Single,
    Group,
}

impl Mode {
    fn id(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Group => "group",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Reaction {
    ThumbsUp,
    ThumbsDown,
    Heart,
    Joy,
    Tada,
    Clap,
}

impl Reaction {
    fn id(self) -> &'static str {
        match self {
            Self::ThumbsUp => "thumbs_up",
            Self::ThumbsDown => "thumbs_down",
            Self::Heart => "heart",
            Self::Joy => "joy",
            Self::Tada => "tada",
            Self::Clap => "clap",
        }
    }
}

#[derive(Debug, Subcommand)]
enum CameraCommand {
    /// List available cameras (the active one is marked).
    List,
    /// Select a camera by id or /dev/videoN path.
    Select { id: String },
    /// Show the virtual-camera node info.
    Info,
}

#[derive(Debug, Subcommand)]
enum AutostartCommand {
    /// Start the daemon automatically with the graphical session.
    Enable,
    /// Stop starting the daemon automatically.
    Disable,
    /// Show whether autostart is enabled and the daemon is running.
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // `autostart` manages the systemd unit and never needs the daemon running,
    // so handle it before connecting / auto-starting.
    if let Command::Autostart { command } = &cli.command {
        return autostart(command);
    }

    let conn = Connection::session().await?;
    // Every other command talks to the daemon; bring it up if it isn't running.
    ensure_daemon(&conn).await;

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
        Command::Effects => list_effects(&conn).await?,
        Command::Enable { effect } => {
            effects_proxy(&conn)
                .await?
                .call::<_, _, ()>("SetEnabled", &(effect.id(), true))
                .await?;
            show_effect(&conn, effect.id()).await?;
        }
        Command::Disable { effect } => {
            effects_proxy(&conn)
                .await?
                .call::<_, _, ()>("SetEnabled", &(effect.id(), false))
                .await?;
            show_effect(&conn, effect.id()).await?;
        }
        Command::Toggle { effect } => toggle(&conn, effect.id()).await?,

        Command::CenterStage {
            toggle,
            zoom,
            mode,
            crop_top,
            crop_bottom,
            crop_left,
            crop_right,
        } => {
            let effects = effects_proxy(&conn).await?;
            let id = "center_stage";
            let mut changed = apply_enabled(&effects, id, toggle.resolve()).await?;
            if let Some(z) = zoom {
                set_param_str(&effects, id, "zoom", z.id()).await?;
                changed = true;
            }
            if let Some(m) = mode {
                set_param_str(&effects, id, "mode", m.id()).await?;
                changed = true;
            }
            for (key, v) in [
                ("crop_top", crop_top),
                ("crop_bottom", crop_bottom),
                ("crop_left", crop_left),
                ("crop_right", crop_right),
            ] {
                if let Some(v) = v {
                    set_param_u32(&effects, id, key, v).await?;
                    changed = true;
                }
            }
            let _ = changed;
            show_effect(&conn, id).await?;
        }

        Command::PortraitBlur { toggle, strength } => {
            let effects = effects_proxy(&conn).await?;
            let id = "portrait_blur";
            apply_enabled(&effects, id, toggle.resolve()).await?;
            if let Some(s) = strength {
                set_param_u32(&effects, id, "strength", s as u32).await?;
            }
            show_effect(&conn, id).await?;
        }

        Command::Background { value, off } => {
            let effects = effects_proxy(&conn).await?;
            let id = "bg_replace";
            match value.as_deref() {
                Some("none") => {
                    set_param_str(&effects, id, "background", "").await?;
                    set_enabled(&effects, id, false).await?;
                }
                Some(bg) => {
                    set_param_str(&effects, id, "background", bg).await?;
                    set_enabled(&effects, id, !off).await?;
                }
                None => {
                    if off {
                        set_enabled(&effects, id, false).await?;
                    }
                }
            }
            show_effect(&conn, id).await?;
        }

        Command::StudioLight {
            toggle,
            intensity,
            brightness,
            contrast,
        } => {
            let effects = effects_proxy(&conn).await?;
            let id = "studio_light";
            apply_enabled(&effects, id, toggle.resolve()).await?;
            if let Some(v) = intensity {
                set_param_u32(&effects, id, "intensity", v as u32).await?;
            }
            if let Some(v) = brightness {
                set_param_i32(&effects, id, "brightness", v as i32).await?;
            }
            if let Some(v) = contrast {
                set_param_u32(&effects, id, "contrast", v as u32).await?;
            }
            show_effect(&conn, id).await?;
        }

        Command::Reactions { toggle } => {
            let effects = effects_proxy(&conn).await?;
            let id = "reactions";
            apply_enabled(&effects, id, toggle.resolve()).await?;
            show_effect(&conn, id).await?;
        }
        Command::React { reaction } => {
            effects_proxy(&conn)
                .await?
                .call::<_, _, ()>("TriggerReaction", &(reaction.id()))
                .await?;
            println!("fired {}", reaction.id());
        }

        Command::Set { assignment, value } => set_param(&conn, &assignment, &value).await?,
        Command::Camera { command } => camera(&conn, command).await?,
        // Handled before the daemon connection above.
        Command::Autostart { .. } => unreachable!(),
        Command::Watch => watch(&conn).await?,
    }

    Ok(())
}

/// Enable/disable/report the daemon's systemd user unit.
fn autostart(command: &AutostartCommand) -> anyhow::Result<()> {
    match command {
        AutostartCommand::Enable => {
            shared::systemd::set_enabled(true).map_err(|e| anyhow::anyhow!(e))?;
            println!(
                "autostart enabled: {} will start with the graphical session",
                shared::systemd::UNIT
            );
        }
        AutostartCommand::Disable => {
            shared::systemd::set_enabled(false).map_err(|e| anyhow::anyhow!(e))?;
            println!("autostart disabled");
        }
        AutostartCommand::Status => {
            println!(
                "autostart: {}",
                if shared::systemd::is_enabled() {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            println!(
                "daemon:    {}",
                if shared::systemd::is_active() {
                    "running"
                } else {
                    "stopped"
                }
            );
        }
    }
    Ok(())
}

/// Start the daemon if its bus name has no owner. Prefers the systemd unit;
/// if that fails (e.g. unit not installed in a dev tree), proceeds and lets the
/// subsequent D-Bus method call auto-activate it.
async fn ensure_daemon(conn: &Connection) {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else {
        return;
    };
    let Ok(name) = SERVICE_NAME.try_into() else {
        return;
    };
    if dbus.name_has_owner(name).await.unwrap_or(false) {
        return;
    }
    if let Err(err) = shared::systemd::start() {
        tracing::warn!(%err, "systemctl start failed; relying on D-Bus activation");
    }
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

async fn list_effects(conn: &Connection) -> anyhow::Result<()> {
    let effects = effects_proxy(conn).await?;
    let ids: Vec<String> = effects.call("ListEffects", &()).await?;
    for id in ids {
        let params: VariantMap = effects.call("GetParams", &(id.as_str())).await?;
        let on = params
            .get("enabled")
            .and_then(value_as_bool)
            .unwrap_or(false);
        let mark = if on { "on " } else { "off" };
        println!("[{mark}] {id}");
    }
    Ok(())
}

async fn show_effect(conn: &Connection, id: &str) -> anyhow::Result<()> {
    let effects = effects_proxy(conn).await?;
    let params: VariantMap = effects.call("GetParams", &(id)).await?;
    println!("{id}: {}", format_params(&params));
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

/// Apply an optional enable/disable, returning whether anything was sent.
async fn apply_enabled(effects: &Proxy<'_>, id: &str, on: Option<bool>) -> anyhow::Result<bool> {
    if let Some(on) = on {
        set_enabled(effects, id, on).await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn set_enabled(effects: &Proxy<'_>, id: &str, on: bool) -> anyhow::Result<()> {
    effects.call::<_, _, ()>("SetEnabled", &(id, on)).await?;
    Ok(())
}

async fn set_param_u32(effects: &Proxy<'_>, id: &str, key: &str, v: u32) -> anyhow::Result<()> {
    let value: Value<'_> = Value::U32(v);
    effects
        .call::<_, _, ()>("SetParam", &(id, key, value))
        .await?;
    Ok(())
}

async fn set_param_i32(effects: &Proxy<'_>, id: &str, key: &str, v: i32) -> anyhow::Result<()> {
    let value: Value<'_> = Value::I32(v);
    effects
        .call::<_, _, ()>("SetParam", &(id, key, value))
        .await?;
    Ok(())
}

async fn set_param_str(effects: &Proxy<'_>, id: &str, key: &str, v: &str) -> anyhow::Result<()> {
    let value: Value<'_> = Value::Str(Str::from(v.to_owned()));
    effects
        .call::<_, _, ()>("SetParam", &(id, key, value))
        .await?;
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
                let active = camera
                    .get("active")
                    .and_then(value_as_bool)
                    .unwrap_or(false);
                let mark = if active { "* " } else { "  " };
                println!("{mark}{id}\t{name}");
            }
        }
        CameraCommand::Select { id } => {
            devices
                .call::<_, _, ()>("SelectCamera", &(id.as_str()))
                .await?
        }
        CameraCommand::Info => {
            let info = devices
                .get_property::<VariantMap>("VirtualCameraInfo")
                .await
                .unwrap_or_default();
            println!("{}", format_params(&info));
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
