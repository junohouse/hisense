//! Hisense (VIDAA) TVs, over the MQTT broker built into the set — the same one its own Remote
//! NOW app talks to, on port 36669.
//!
//! ```text
//!   PUBLISH /remoteapp/tv/remote_service/Juno/actions/sendkey       KEY_POWER, KEY_VOLUMEUP, ...
//!   PUBLISH /remoteapp/tv/ui_service/Juno/actions/gettvstate        ask what it is doing
//!   PUBLISH /remoteapp/tv/ui_service/Juno/actions/sourcelist        ask what inputs it has
//!   PUBLISH /remoteapp/tv/ui_service/Juno/actions/applist           ask what channels it has
//!   PUBLISH /remoteapp/tv/ui_service/Juno/actions/changesource      {"sourceid":..,"sourcename":..}
//!   PUBLISH /remoteapp/tv/ui_service/Juno/actions/launchapp         the app's own applist entry, echoed back
//!   PUBLISH /remoteapp/tv/platform_service/Juno/actions/changevolume  a bare number, 0-100
//!   PUBLISH /remoteapp/tv/ui_service/Juno/actions/authenticationcode {"authNum":"1234"}
//!   SUBSCRIBE /remoteapp/mobile/broadcast/#, /remoteapp/mobile/Juno/#
//! ```
//!
//! `Juno` is a fixed, arbitrary label folded into the topic path — every third-party tool that
//! talks to one of these picks its own (`HomeAssistant`, `AutoHTPC`); the TV echoes it back on
//! replies scoped to whoever asked, and there is nothing to gain by making it configurable.
//! It is unrelated to the MQTT client id core connects as (`[[property]] "Client id"`), which
//! is what a broker actually keys a session on.
//!
//! Messages are told apart by shape, not by topic: a sourcelist and an applist are both a JSON
//! array on a reply topic this driver does not fully control the naming of, and the two shapes
//! never collide — one is keyed by `sourceid`, the other by `name`. Recognizing the field is
//! more durable than recognizing the path.
//!
//! # No auto-discovery
//!
//! The broker gives away nothing before it is spoken to — nothing to probe for, nothing an SSDP
//! or mDNS answer would confirm is a Hisense rather than any other MQTT device on port 36669. So
//! setup just asks for the address and the MAC, the same two things its own Settings → Network
//! screen shows.
//!
//! # "on" is Wake-on-LAN
//!
//! The broker is not running while the TV is asleep, so there is nothing for a keypress to
//! reach. `on` sends a magic packet instead; `off` and `power_toggle` are the one physical key
//! this remote has, `KEY_POWER`.

use driver_sdk::*;
use driver_sdk::Value;

#[derive(Default)]
pub struct Hisense;

/// The topic-path label — see the module doc. Fixed because one string works for every unit;
/// nothing here is a pairing secret or a per-install identity.
const CLIENT: &str = "Juno";

const MEDIA: LocalId = 1;
const TV: LocalId = 2;

/// A connection id for one of the TV's sources, derived from the name it calls it.
///
/// From the name and not from `sourceid`, which is per model — a real set reports HDMI 1 as
/// source 3 — and not from list order either, since a project remembers what an installer
/// wired by this number and a firmware update must not move somebody's cabling.
fn connection_id(sourcename: &str) -> Option<LocalId> {
    let name = sourcename.trim();
    if let Some(n) = name.strip_prefix("HDMI") {
        return n.trim().parse::<LocalId>().ok().filter(|n| (1..=99).contains(n)).map(|n| 1000 + n);
    }
    match name.to_ascii_uppercase().as_str() {
        "AV" | "COMPOSITE" => Some(1101),
        "COMPONENT" => Some(1102),
        "TV" => Some(1201),
        // Anything else this model happens to list. Reported rather than dropped would mean
        // inventing an id for a name nobody here has seen, and ids have to be stable.
        _ => None,
    }
}

/// What kind of cable a source takes, for the pathfinder's own vocabulary.
fn signal_class(sourcename: &str) -> &'static str {
    match connection_id(sourcename) {
        Some(1101) => "COMPOSITE",
        Some(1102) => "COMPONENT",
        Some(1201) => "RF_UHF_VHF",
        _ => "HDMI",
    }
}

impl Hisense {
    fn publish(service: &str, action: &str, payload: String) -> HostCall {
        HostCall::Publish {
            topic: format!("/remoteapp/tv/{service}/{CLIENT}/actions/{action}"),
            payload,
        }
    }

    fn send_key(key: &str) -> HostCall {
        Self::publish("remote_service", "sendkey", key.into())
    }

    fn get_state() -> HostCall {
        Self::publish("ui_service", "gettvstate", String::new())
    }

    fn get_sources() -> HostCall {
        Self::publish("ui_service", "sourcelist", String::new())
    }

    fn get_apps() -> HostCall {
        Self::publish("ui_service", "applist", String::new())
    }

    /// The cached sourcelist reply, or empty before the first one has arrived.
    fn sources(inst: &Instance) -> Vec<Value> {
        inst.scratch.get("sources").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    /// The cached applist reply, kept as the TV's own raw entries: launching an app means
    /// echoing one straight back, so there is nothing to gain by reshaping them on the way in.
    fn apps(inst: &Instance) -> Vec<Value> {
        inst.scratch.get("apps").and_then(Value::as_array).cloned().unwrap_or_default()
    }
}

fn normalize(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Exact, then prefix, then substring: "netflix" -> "Netflix", "prime" -> "Prime Video". People
/// do not say an app's full installed name. Returns the best-matching entry's index.
fn best_match<'a>(want: &str, names: impl Iterator<Item = &'a str>) -> Option<usize> {
    let want_norm = normalize(want);
    let mut best: Option<(u8, usize)> = None;
    for (i, name) in names.enumerate() {
        let n = normalize(name);
        let rank = if n == want_norm {
            0
        } else if n.starts_with(&want_norm) || want_norm.starts_with(&n) {
            1
        } else if n.contains(&want_norm) || want_norm.contains(&n) {
            2
        } else {
            continue;
        };
        if best.is_none_or(|(r, _)| rank < r) {
            best = Some((rank, i));
        }
    }
    best.map(|(_, i)| i)
}

impl DriverModule for Hisense {
    fn discover(&self, _driver_id: &str, state: &Value, input: &Args) -> (SetupStep, Value) {
        self.flow(state, input)
    }

    fn setup(&self, _driver_id: &str, state: &Value, input: &Args) -> (SetupStep, Value) {
        self.flow(state, input)
    }

    fn on_action(&self, _inst: &mut Instance, action: &str, args: &Args) -> Vec<HostCall> {
        match action {
            "pair" => {
                let Some(code) = args.get("code").and_then(Value::as_str) else {
                    return vec![HostCall::warn("hisense: pair needs the code shown on the TV")];
                };
                vec![Hisense::publish(
                    "ui_service",
                    "authenticationcode",
                    json!({ "authNum": code }).to_string(),
                )]
            }
            other => vec![HostCall::warn(format!("hisense: unknown action `{other}`"))],
        }
    }

    fn on_command(
        &self,
        inst: &mut Instance,
        proxy: LocalId,
        cmd: &str,
        args: &Args,
    ) -> Vec<HostCall> {
        // Wakes a TV that is fully asleep, which is exactly when nothing else here would reach
        // it — the broker is not running, so this is the one command that needs no connection.
        if (proxy, cmd) == (TV, "on") {
            let Some(mac) = inst.property("MAC address").as_str().filter(|s| !s.is_empty()) else {
                return vec![HostCall::warn("hisense: set the MAC address on this device first")];
            };
            let mut a = Args::new();
            a.insert("on".into(), json!(true));
            return vec![HostCall::Wol { mac: mac.into() }, HostCall::notify(TV, "power_changed", a)];
        }

        if inst.property("Address").as_str().filter(|s| !s.is_empty()).is_none() {
            return vec![HostCall::warn("hisense: set the Address on this device first")];
        }

        if cmd == "launch_app" {
            let Some(want) = args.get("app").and_then(Value::as_str) else {
                return vec![HostCall::warn("hisense: launch_app needs an app name")];
            };
            let apps = Hisense::apps(inst);
            let names = apps.iter().map(|a| a.get("name").and_then(Value::as_str).unwrap_or(""));
            let Some(i) = best_match(want, names) else {
                let known: Vec<&str> =
                    apps.iter().filter_map(|a| a.get("name").and_then(Value::as_str)).collect();
                return vec![HostCall::warn(format!(
                    "hisense: no app matching `{want}`; installed: {}",
                    known.join(", ")
                ))];
            };
            let app = &apps[i];
            let name = app.get("name").and_then(Value::as_str).unwrap_or(want).to_string();
            let mut a = Args::new();
            a.insert("app".into(), json!(name));
            return vec![
                Hisense::publish("ui_service", "launchapp", app.to_string()),
                HostCall::notify(MEDIA, "app_changed", a),
            ];
        }

        let key = match (proxy, cmd) {
            (_, "play") => "KEY_PLAY",
            (_, "pause") => "KEY_PAUSE",
            (_, "stop") => "KEY_STOP",
            (_, "skip_forward") | (_, "scan_forward") => "KEY_FORWARDS",
            (_, "skip_back") | (_, "scan_reverse") => "KEY_BACKS",

            (TV, "off") | (TV, "power_toggle") => "KEY_POWER",
            (TV, "volume_up") => "KEY_VOLUMEUP",
            (TV, "volume_down") => "KEY_VOLUMEDOWN",
            (TV, "mute_toggle") => "KEY_MUTE",

            (TV, "set_volume") => {
                let Some(level) = args.get("level").and_then(Value::as_u64) else {
                    return vec![HostCall::warn("hisense: set_volume needs a level")];
                };
                let mut a = Args::new();
                a.insert("level".into(), json!(level));
                return vec![
                    Hisense::publish("platform_service", "changevolume", level.to_string()),
                    HostCall::notify(TV, "volume_changed", a),
                ];
            }

            (TV, "set_input") => {
                let Some(conn) = args.get("connection").and_then(Value::as_u64) else {
                    return vec![HostCall::warn("hisense: set_input needs a connection")];
                };
                // Against what the TV reported, so the name sent back is the one it uses —
                // `sourceid` is per model and cannot be derived.
                let sources = Hisense::sources(inst);
                let Some(source) = sources.iter().find(|s| {
                    s.get("sourcename")
                        .and_then(Value::as_str)
                        .and_then(connection_id)
                        .is_some_and(|id| u64::from(id) == conn)
                }) else {
                    return vec![HostCall::warn(format!(
                        "hisense: this TV has not reported a source for connection {conn} yet"
                    ))];
                };
                let sourceid = source.get("sourceid").and_then(Value::as_str).unwrap_or_default();
                let name = source.get("sourcename").and_then(Value::as_str).unwrap_or_default();
                let payload = json!({ "sourceid": sourceid, "sourcename": name }).to_string();
                let mut a = Args::new();
                a.insert("connection".into(), json!(conn));
                return vec![
                    Hisense::publish("ui_service", "changesource", payload),
                    HostCall::notify(TV, "input_changed", a),
                ];
            }

            (_, "dpad") => {
                let Some(k) = args.get("key").and_then(Value::as_str) else {
                    return vec![HostCall::warn("hisense: dpad needs a key")];
                };
                match k {
                    "up" => "KEY_UP",
                    "down" => "KEY_DOWN",
                    "left" => "KEY_LEFT",
                    "right" => "KEY_RIGHT",
                    "select" => "KEY_OK",
                    "back" => "KEY_RETURNS",
                    "home" => "KEY_HOME",
                    "menu" => "KEY_MENU",
                    other => return vec![HostCall::warn(format!("hisense: no key `{other}`"))],
                }
            }

            (_, other) => return vec![HostCall::warn(format!("hisense: unhandled `{other}`"))],
        };

        let mut out = vec![Hisense::send_key(key)];
        match cmd {
            "play" => {
                let mut a = Args::new();
                a.insert("state".into(), json!("playing"));
                out.push(HostCall::notify(MEDIA, "transport_changed", a));
            }
            "pause" => {
                let mut a = Args::new();
                a.insert("state".into(), json!("paused"));
                out.push(HostCall::notify(MEDIA, "transport_changed", a));
            }
            "stop" => {
                let mut a = Args::new();
                a.insert("state".into(), json!("stopped"));
                out.push(HostCall::notify(MEDIA, "transport_changed", a));
            }
            "off" => {
                let mut a = Args::new();
                a.insert("on".into(), json!(false));
                out.push(HostCall::notify(TV, "power_changed", a));
            }
            // power_toggle and mute_toggle: no optimiztic notify. There is only the one key,
            // so which way it just went is a guess this driver is not in a better position to
            // make than waiting for the TV to say so on its own.
            _ => {}
        }
        out
    }

    fn on_event(
        &self,
        inst: &mut Instance,
        _control: LocalId,
        note: &str,
        args: &Args,
    ) -> Vec<HostCall> {
        if note != "mqtt" {
            return Vec::new();
        }
        let Some(data) = args.get("data").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Ok(msg) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };

        // The reply to `authenticationcode`. Anything other than success means the code was
        // wrong or has expired — the TV shows a fresh one each time, so say to look again.
        if let Some(result) = msg.get("result").and_then(Value::as_i64) {
            return if result == 1 {
                Vec::new()
            } else {
                vec![HostCall::warn(
                    "hisense: that code was not accepted — check the TV screen for a current \
                     one and use `pair` again",
                )]
            };
        }

        // A volume report, from `getvolume` or pushed on its own after a hardware remote
        // changes it.
        if let Some(level) = msg.get("volume_value").and_then(Value::as_u64) {
            let mut a = Args::new();
            a.insert("level".into(), json!(level));
            return vec![HostCall::notify(TV, "volume_changed", a)];
        }

        // Told apart from an applist by field, not by topic — see the module doc.
        if let Some(arr) = msg.as_array().filter(|a| !a.is_empty()) {
            if arr.iter().all(|e| e.get("sourceid").is_some()) {
                inst.scratch.insert("sources".into(), msg.clone());
                // What this set actually has — the only place a Hisense's connections come
                // from, since the manifest declares none. A model with three HDMI, or with a
                // component input, is the ordinary case rather than the exception, so there is
                // no product-line guess worth writing down. See `HostCall::Connections`.
                let connections: Vec<ConnectionDecl> = arr
                    .iter()
                    .filter_map(|s| {
                        let name = s.get("sourcename").and_then(Value::as_str)?;
                        Some(ConnectionDecl {
                            id: connection_id(name)?,
                            proxy: TV,
                            dir: Direction::Consumer,
                            class: signal_class(name).into(),
                            name: name.trim().to_string(),
                        })
                    })
                    .collect();
                return vec![HostCall::Connections { connections }];
            }
            if arr.iter().all(|e| e.get("name").is_some()) {
                inst.scratch.insert("apps".into(), msg.clone());
                let names: Vec<&str> =
                    arr.iter().filter_map(|a| a.get("name").and_then(Value::as_str)).collect();
                let icons: Vec<&str> = arr
                    .iter()
                    .map(|a| a.get("iconUrl").and_then(Value::as_str).unwrap_or(""))
                    .collect();
                let mut a = Args::new();
                a.insert("apps".into(), json!(names));
                a.insert("app_icons".into(), json!(icons));
                return vec![HostCall::notify(MEDIA, "apps_changed", a)];
            }
            return Vec::new();
        }

        // `gettvstate`'s reply, and every unsolicited `sourceswitch` push: whatever this TV is
        // showing right now, named the same way a sourcelist entry names it.
        if let Some(name) = msg.get("sourcename").and_then(Value::as_str)
            && let Some(id) = connection_id(name)
        {
            let mut a = Args::new();
            a.insert("connection".into(), json!(id));
            return vec![HostCall::notify(TV, "input_changed", a)];
        }

        Vec::new()
    }

    fn on_bind(&self, _inst: &mut Instance) -> Vec<HostCall> {
        let mut out = Vec::new();
        let mut a = Args::new();
        a.insert("online".into(), json!(true));
        out.push(HostCall::notify(MEDIA, "online_changed", a));
        out.push(HostCall::Subscribe { topic: "/remoteapp/mobile/broadcast/#".into() });
        out.push(HostCall::Subscribe { topic: format!("/remoteapp/mobile/{CLIENT}/#") });
        out.push(Hisense::get_state());
        out.push(Hisense::get_sources());
        out.push(Hisense::get_apps());
        out
    }
}

// ---------------------------------------------------------------------------------------
// Setup flow
// ---------------------------------------------------------------------------------------

impl Hisense {
    /// Two fields, no verification: the broker answers nothing until it is spoken to as a
    /// paired client, so there is no honest way to confirm a Hisense is really at this address
    /// before adding it — see the module doc.
    fn flow(&self, state: &Value, input: &Args) -> (SetupStep, Value) {
        let phase = state.get("phase").and_then(Value::as_str).unwrap_or("start");
        match phase {
            "start" => (
                SetupStep::Form {
                    title: "Add a Hisense TV".into(),
                    body: "Both are on the TV itself: Settings → Network → check the network \
                           status. The MAC is what lets `on` reach it while it is asleep."
                        .into(),
                    fields: vec![
                        Field {
                            name: "address".into(),
                            label: "Address".into(),
                            kind: "string".into(),
                            help: "for example 192.168.1.42".into(),
                            default: None,
                            options: Vec::new(),
                            required: true,
                        },
                        Field {
                            name: "mac".into(),
                            label: "MAC address".into(),
                            kind: "string".into(),
                            help: "for example AA:BB:CC:DD:EE:FF".into(),
                            default: None,
                            options: Vec::new(),
                            required: true,
                        },
                    ],
                },
                json!({ "phase": "entered" }),
            ),

            "entered" => {
                let address =
                    input.get("address").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                let mac = input.get("mac").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if address.is_empty() || mac.is_empty() {
                    return (
                        SetupStep::Failed { reason: "both the address and the MAC are needed".into() },
                        Value::Null,
                    );
                }
                (
                    SetupStep::Choose {
                        title: "Add this TV".into(),
                        body: "This cannot be confirmed before adding it — if a pairing code \
                               does not appear on the TV's screen once this is done, the \
                               address is wrong."
                            .into(),
                        options: vec![Candidate {
                            label: format!("Hisense TV ({address})"),
                            kind: "Hisense TV".into(),
                            driver_id: "hisense.tv".into(),
                            properties: [
                                ("Address".to_string(), json!(address)),
                                ("MAC address".to_string(), json!(mac)),
                            ]
                            .into_iter()
                            .collect(),
                            verified: "not checked — its broker says nothing until paired".into(),
                            ..Default::default()
                        }],
                        multiple: false,
                    },
                    json!({ "phase": "chosen" }),
                )
            }

            "chosen" => {
                let devices: Vec<Candidate> = input
                    .get("chosen")
                    .and_then(|c| driver_sdk::serde_json::from_value(c.clone()).ok())
                    .unwrap_or_default();
                (SetupStep::done(devices), Value::Null)
            }

            other => (
                SetupStep::Failed { reason: format!("unknown setup phase `{other}`") },
                Value::Null,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mqtt(data: &str) -> Args {
        let mut a = Args::new();
        a.insert("data".into(), json!(data));
        a
    }

    #[test]
    fn app_names_match_the_way_people_say_them() {
        let names = ["Netflix", "Prime Video", "Disney+", "The Roku Channel"];
        assert_eq!(best_match("netflix", names.into_iter()), Some(0));
        assert_eq!(best_match("prime", names.into_iter()), Some(1));
        assert_eq!(best_match("disney", names.into_iter()), Some(2));
        assert_eq!(best_match("hbo", names.into_iter()), None);
    }

    /// A sourcelist and an applist arrive on topics this driver does not fully control the
    /// naming of, so telling them apart by field is what the module doc promises — this is the
    /// promise checked.
    #[test]
    fn a_sourcelist_and_an_applist_are_told_apart_by_field_not_topic() {
        let driver = Hisense;
        let mut inst = Instance::default();

        let sources = r#"[{"sourceid":"0","sourcename":"TV","displayname":"TV"},
                           {"sourceid":"4","sourcename":"HDMI 2","displayname":"HDMI 2"}]"#;
        let calls = driver.on_event(&mut inst, 0, "mqtt", &mqtt(sources));
        let [HostCall::Connections { connections }] = calls.as_slice() else {
            panic!("a sourcelist says what inputs this set has, got {calls:?}");
        };
        assert_eq!(connections.len(), 2);
        assert_eq!(Hisense::sources(&inst).len(), 2);

        let apps = r#"[{"name":"Netflix","urlType":37,"url":"netflix"},
                        {"name":"YouTube","urlType":37,"url":"youtube"}]"#;
        let calls = driver.on_event(&mut inst, 0, "mqtt", &mqtt(apps));
        let [HostCall::Notify { name, args, .. }] = calls.as_slice() else {
            panic!("expected one apps_changed notification, got {calls:?}");
        };
        assert_eq!(name, "apps_changed");
        assert_eq!(args.get("apps").unwrap(), &json!(["Netflix", "YouTube"]));
    }

    #[test]
    fn a_volume_report_becomes_a_volume_changed_notification() {
        let driver = Hisense;
        let mut inst = Instance::default();
        let calls = driver.on_event(&mut inst, 0, "mqtt", &mqtt(r#"{"volume_type":0,"volume_value":42}"#));
        let [HostCall::Notify { name, args, .. }] = calls.as_slice() else {
            panic!("expected one volume_changed notification, got {calls:?}");
        };
        assert_eq!(name, "volume_changed");
        assert_eq!(args.get("level").unwrap(), &json!(42));
    }

    /// `set_input` names a connection by its manifest id; the TV only understands its own
    /// `sourceid`. Nothing to translate with before the first sourcelist has arrived.
    #[test]
    fn set_input_warns_before_any_sourcelist_has_been_seen() {
        let driver = Hisense;
        let mut inst = Instance::default();
        inst.properties.insert("Address".into(), json!("10.0.0.5"));
        let mut a = Args::new();
        a.insert("connection".into(), json!(1002u64));
        let calls = driver.on_command(&mut inst, TV, "set_input", &a);
        assert!(
            matches!(calls.as_slice(), [HostCall::Log { level, .. }] if level == "warn"),
            "expected a warning, got {calls:?}"
        );
    }

    #[test]
    fn set_input_publishes_the_tvs_own_sourceid_once_known() {
        let driver = Hisense;
        let mut inst = Instance::default();
        inst.properties.insert("Address".into(), json!("10.0.0.5"));
        inst.scratch.insert(
            "sources".into(),
            json!([{"sourceid": "4", "sourcename": "HDMI 2"}]),
        );
        let mut a = Args::new();
        a.insert("connection".into(), json!(1002u64));
        let calls = driver.on_command(&mut inst, TV, "set_input", &a);
        let [HostCall::Publish { topic, payload }, HostCall::Notify { .. }] = calls.as_slice()
        else {
            panic!("expected a publish and a notify, got {calls:?}");
        };
        assert!(topic.ends_with("/actions/changesource"));
        assert!(payload.contains(r#""sourceid":"4""#), "{payload}");
    }

    /// The one command that has to work with the TV fully asleep — see the module doc — so it
    /// must never depend on the MQTT connection being reachable.
    #[test]
    fn turning_on_sends_a_magic_packet_not_a_keypress() {
        let driver = Hisense;
        let mut inst = Instance::default();
        inst.properties.insert("MAC address".into(), json!("AA:BB:CC:DD:EE:FF"));
        let calls = driver.on_command(&mut inst, TV, "on", &Args::new());
        assert!(
            matches!(&calls[0], HostCall::Wol { mac } if mac == "AA:BB:CC:DD:EE:FF"),
            "expected a WoL packet first, got {calls:?}"
        );
    }

    #[test]
    fn turning_on_without_a_mac_set_warns_instead_of_doing_nothing_silently() {
        let driver = Hisense;
        let mut inst = Instance::default();
        let calls = driver.on_command(&mut inst, TV, "on", &Args::new());
        assert!(matches!(calls.as_slice(), [HostCall::Log { level, .. }] if level == "warn"));
    }
    /// A real sourcelist — this is the shape one arrives in — is the whole of what the set in
    /// the room has, since the manifest declares nothing. `sourceid` is per model (HDMI 1 is
    /// source 3 here), which is why ids come from the name rather than from it.
    #[test]
    fn a_sourcelist_reports_what_this_set_actually_has() {
        let driver = Hisense;
        let mut inst = Instance::default();
        let sources = r#"[{"sourceid":"4","sourcename":"HDMI 2","displayname":"HDMI 2"},
                          {"sourceid":"0","sourcename":"TV","displayname":"TV"},
                          {"sourceid":"3","sourcename":"HDMI 1","displayname":"HDMI 1"},
                          {"sourceid":"2","sourcename":"COMPONENT","displayname":"COMPONENT"},
                          {"sourceid":"1","sourcename":"AV","displayname":"AV"}]"#;
        let calls = driver.on_event(&mut inst, 0, "mqtt", &mqtt(sources));
        let [HostCall::Connections { connections }] = calls.as_slice() else {
            panic!("expected one Connections call, got {calls:?}");
        };

        let ids: Vec<LocalId> = connections.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![1002, 1201, 1001, 1102, 1101]);
        assert!(
            !ids.contains(&1003) && !ids.contains(&1004),
            "the manifest's third and fourth HDMI are phantoms on this set"
        );
        assert!(connections.iter().all(|c| c.dir == Direction::Consumer && c.proxy == TV));
        assert_eq!(connections.iter().find(|c| c.id == 1002).unwrap().class, "HDMI");
        assert_eq!(connections.iter().find(|c| c.id == 1201).unwrap().class, "RF_UHF_VHF");
        assert_eq!(connections.iter().find(|c| c.id == 1101).unwrap().class, "COMPOSITE");
    }

    /// Ids come from the name, so a set that lists its sources in another order — or numbers
    /// them differently, which every model does — keeps the ids a project was wired against.
    #[test]
    fn connection_ids_come_from_the_name_not_the_sourceid() {
        assert_eq!(connection_id("HDMI 1"), Some(1001));
        assert_eq!(connection_id("HDMI4"), Some(1004), "spacing varies between models");
        assert_eq!(connection_id("AV"), Some(1101));
        assert_eq!(connection_id("TV"), Some(1201));
        assert_eq!(connection_id("Chromecast"), None, "an unknown name gets no invented id");
    }
}

export_driver!(Hisense);
