use shared::dbus::{
    DAEMON_INTERFACE, DEVICES_INTERFACE, EFFECTS_INTERFACE, OBJECT_PATH, SERVICE_NAME,
};

#[test]
fn dbus_contract_constants_match_prd() {
    assert_eq!(SERVICE_NAME, "in.co.funinkina.Daemon");
    assert_eq!(OBJECT_PATH, "/in/co/funinkina/Daemon");
    assert_eq!(DAEMON_INTERFACE, "in.co.funinkina.Daemon1");
    assert_eq!(EFFECTS_INTERFACE, "in.co.funinkina.Effects1");
    assert_eq!(DEVICES_INTERFACE, "in.co.funinkina.Devices1");
}

#[tokio::test]
#[ignore = "requires a running openeffectsd instance on the session bus"]
async fn daemon_status_roundtrip() -> anyhow::Result<()> {
    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(&conn, SERVICE_NAME, OBJECT_PATH, DAEMON_INTERFACE).await?;
    let status = proxy.get_property::<String>("Status").await?;
    assert!(matches!(
        status.as_str(),
        "stopped" | "starting" | "running" | "idle" | "error"
    ));
    Ok(())
}
