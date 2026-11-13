use bevy::window::{MonitorSelection, VideoModeSelection, WindowMode as BevyMode, WindowPosition};

use super::*;

fn mode(w: u32, h: u32, hz: u32, depth: u16) -> bevy::window::VideoMode {
    bevy::window::VideoMode { physical_size: UVec2::new(w, h), bit_depth: depth, refresh_rate_millihertz: hz }
}

fn screens<'a>(modes: &'a [bevy::window::VideoMode]) -> Vec<Screen<'a>> {
    vec![
        Screen {
            entity: Entity::from_raw_u32(1).unwrap(),
            name: Some("DP-1"),
            primary: true,
            size: UVec2::new(1920, 1080),
            modes: &[],
        },
        Screen {
            entity: Entity::from_raw_u32(2).unwrap(),
            name: Some("DP-2"),
            primary: false,
            size: UVec2::new(3440, 1440),
            modes,
        },
    ]
}

fn settings(mode: WindowMode, monitor: Option<&str>, video: Option<VideoMode>) -> DisplaySettings {
    DisplaySettings {
        mode,
        fullscreen: mode,
        monitor: monitor.map(str::to_string),
        video_mode: video,
        window: None,
        render_height: None,
    }
}

#[test]
fn the_settings_resolve_against_the_monitors_and_the_windowing_system() {
    let modes = [mode(3440, 1440, 100_000, 24), mode(3440, 1440, 100_000, 32), mode(2560, 1080, 60_000, 32)];
    let list = screens(&modes);
    let (dp1, dp2) = (list[0].entity, list[1].entity);
    let uw = VideoMode { width: 3440, height: 1440, refresh_mhz: 100_000 };
    let r = resolve(&settings(WindowMode::Exclusive, Some("DP-2"), Some(uw)), &list, Some(Backend::X11));
    assert_eq!(r.mode, BevyMode::Fullscreen(MonitorSelection::Entity(dp2), VideoModeSelection::Specific(modes[1])));
    assert_eq!((r.monitor, r.effective, r.video, r.notes.len()), (Some(dp2), WindowMode::Exclusive, Some(uw), 0));
    let odd = VideoMode { width: 2560, height: 1080, refresh_mhz: 75_000 };
    let r = resolve(&settings(WindowMode::Exclusive, Some("DP-2"), Some(odd)), &list, Some(Backend::Windows));
    assert_eq!(r.mode, BevyMode::Fullscreen(MonitorSelection::Entity(dp2), VideoModeSelection::Current));
    assert!(r.notes[0].contains("2560x1080@75") && r.notes[0].contains("\"DP-2\""), "{:?}", r.notes);
    let r = resolve(&settings(WindowMode::Exclusive, Some("DP-2"), Some(uw)), &list, Some(Backend::Wayland));
    assert_eq!(
        (r.mode, r.effective),
        (BevyMode::BorderlessFullscreen(MonitorSelection::Entity(dp2)), WindowMode::Borderless)
    );
    assert!(r.notes[0].contains("not available on wayland"), "{:?}", r.notes);
    let r = resolve(&settings(WindowMode::Borderless, Some("HDMI-A-1"), None), &list, Some(Backend::X11));
    assert_eq!((r.mode, r.monitor), (BevyMode::BorderlessFullscreen(MonitorSelection::Entity(dp1)), Some(dp1)));
    assert!(r.notes[0].contains("\"HDMI-A-1\" is not connected") && r.notes[0].contains("\"DP-1\""), "{:?}", r.notes);
    let r = resolve(&settings(WindowMode::Windowed, Some("DP-2"), None), &list, None);
    assert_eq!(
        (r.mode, r.position),
        (BevyMode::Windowed, Some(WindowPosition::Centered(MonitorSelection::Entity(dp2))))
    );
    let r = resolve(&settings(WindowMode::Windowed, None, None), &list, None);
    assert_eq!((r.position, r.monitor, r.notes.len()), (None, Some(dp1), 0));
    let r = resolve(&settings(WindowMode::Borderless, Some("DP-2"), None), &[], Some(Backend::X11));
    assert_eq!(
        (r.mode, r.monitor, r.notes.len()),
        (BevyMode::BorderlessFullscreen(MonitorSelection::Primary), None, 0)
    );
    let long = format!("{}\u{1b}", "x".repeat(300));
    assert!(shown(&long).len() < 140 && !shown("a\u{1b}b").contains('\u{1b}'));
}

#[test]
fn the_options_override_the_files_member_and_window_reads_no_file() {
    let dir = std::env::temp_dir().join(format!("ssx-bevy-display-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    std::fs::write(
        &file,
        r#"{"display": {"mode": "Exclusive", "monitor": "DP-2", "window": {"width": 2560, "height": 1080}}}"#,
    )
    .unwrap();
    let none = DisplayCli::default();
    let s = WindowSpec::new(None, &none, true, Some(&file)).unwrap();
    assert_eq!((s.display.mode, s.display.monitor.as_deref()), (WindowMode::Exclusive, Some("DP-2")));
    assert_eq!((s.width, s.height, s.physical, s.file.as_deref()), (2560, 1080, false, Some(file.as_path())));
    assert!(!s.window("t").visible, "hidden until the monitors are known");
    let cli =
        DisplayCli { window_mode: Some(WindowMode::Windowed), monitor: Some("DP-1".into()), ..Default::default() };
    let s = WindowSpec::new(None, &cli, true, Some(&file)).unwrap();
    assert_eq!(
        (s.display.mode, s.display.fullscreen, s.display.monitor.as_deref()),
        (WindowMode::Windowed, WindowMode::Exclusive, Some("DP-1"))
    );
    let s = WindowSpec::new(Some("1280x720"), &none, true, Some(&file)).unwrap();
    assert_eq!((&s.display, s.file.is_none(), s.physical, s.width), (&DisplaySettings::default(), true, true, 1280));
    assert!(s.window("t").visible);
    assert!(WindowSpec::new(Some("1280x0"), &none, true, None).is_err());
    std::fs::write(&file, "[]").unwrap();
    let s = WindowSpec::new(None, &none, true, Some(&file)).unwrap();
    assert_eq!((s.display, s.notes.len()), (DisplaySettings::default(), 1));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_display_member_is_written_off_the_main_thread_and_kept_at_the_exit() {
    let dir = std::env::temp_dir().join(format!("ssx-bevy-display-keep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    std::fs::write(&file, r#"{"vsync": true}"#).unwrap();
    let base = DisplaySettings::default();
    let mut keeper = keeper::Keeper::start(file.clone(), base.clone()).unwrap();
    let held = std::sync::Arc::new(std::sync::Barrier::new(2));
    let (started, launcher_file) = (held.clone(), file.clone());
    let launcher = std::thread::spawn(move || {
        ssx_assets::settings_file::update_object(&launcher_file, |m| {
            started.wait();
            std::thread::sleep(std::time::Duration::from_millis(400));
            m.insert("vsync".into(), serde_json::Value::Bool(false));
            Ok(())
        })
        .unwrap();
    });
    held.wait();
    let t = std::time::Instant::now();
    for width in [800, 1024, 1600] {
        keeper.keep(DisplaySettings { window: Some((width, 900)), mode: WindowMode::Borderless, ..base.clone() });
    }
    assert!(t.elapsed() < std::time::Duration::from_millis(100), "the requests waited {:?}", t.elapsed());
    launcher.join().unwrap();
    keeper.finish();
    let (kept, notes) = ssx_assets::display::read_member(&file);
    assert_eq!((kept.window, kept.mode, notes.len()), (Some((1600, 900)), WindowMode::Borderless, 0));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains(r#""vsync": false"#) || text.contains(r#""vsync":false"#), "the launcher's change: {text}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_display_options_are_read_strictly() {
    assert_eq!(mode_arg("Borderless"), Ok(WindowMode::Borderless));
    assert!(fullscreen_arg("windowed").is_err());
    assert!(monitor_arg("").is_err() && monitor_arg("a\tb").is_err() && monitor_arg(&"m".repeat(129)).is_err());
    assert_eq!(monitor_arg("--connect=x"), Ok("--connect=x".into()));
    assert_eq!(size_arg("2560x1080"), Ok((2560, 1080)));
    assert!(size_arg("100x100").is_err() && size_arg("9000x900").is_err());
    assert!(video_arg("2560x1080").is_err());
    assert_eq!(render_height_arg("1080"), Ok(1080));
    for bad in ["0", "239", "8193", "1080.0", "-1080", "x"] {
        assert!(render_height_arg(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_render_height_is_set_from_the_command_line() {
    let cli = DisplayCli { render_height: Some(1080), ..Default::default() };
    let s = WindowSpec::new(Some("3840x2160"), &cli, true, None).unwrap();
    assert_eq!(s.display.render_height, Some(1080));
    let s = WindowSpec::new(None, &DisplayCli::default(), true, None).unwrap();
    assert_eq!(s.display.render_height, None);
}

#[test]
fn alt_enter_is_read_in_the_messages_order() {
    use bevy::input::ButtonState::{Pressed, Released};
    let frame = |a: &mut AltEnter, keys: &[(KeyCode, bevy::input::ButtonState, bool)]| {
        a.pressed = false;
        for &(k, s, repeat) in keys {
            a.key(k, s, repeat);
        }
        a.pressed
    };
    let mut a = AltEnter::default();
    let chord = [
        (KeyCode::AltLeft, Pressed, false),
        (KeyCode::Enter, Pressed, false),
        (KeyCode::Enter, Released, false),
        (KeyCode::AltLeft, Released, false),
    ];
    assert!(frame(&mut a, &chord));
    assert!(!frame(&mut a, &[(KeyCode::Enter, Pressed, false)]));
    assert!(!frame(&mut a, &[(KeyCode::Enter, Released, false), (KeyCode::AltRight, Pressed, false)]));
    assert!(frame(&mut a, &[(KeyCode::NumpadEnter, Pressed, false)]));
    assert!(!frame(&mut a, &[(KeyCode::NumpadEnter, Pressed, true)]));
    assert!(!frame(&mut a, &[(KeyCode::AltRight, Released, false), (KeyCode::Enter, Pressed, false)]));
    assert!(!frame(&mut a, &[(KeyCode::Enter, Released, false), (KeyCode::AltLeft, Pressed, false)]));
    a.focus_lost();
    assert!(!frame(&mut a, &[(KeyCode::Enter, Pressed, false)]));
}

#[test]
fn the_toggles_enter_is_taken_out_of_the_keys() {
    use bevy::input::ButtonState::{Pressed, Released};
    use bevy::input::keyboard::{Key, KeyboardInput};
    #[derive(Resource, Default)]
    struct Field(Vec<KeyCode>);
    fn field(alt: Res<AltEnter>, mut keys: MessageReader<KeyboardInput>, mut typed: ResMut<Field>) {
        typed.0 = alt.typed(keys.read_with_id()).filter(|m| m.state == Pressed).map(|m| m.key_code).collect();
    }
    let mut app = App::new();
    app.add_plugins(bevy::input::InputPlugin).init_resource::<Field>().add_systems(Update, field);
    alt_enter::register(&mut app);
    let send = |app: &mut App, keys: &[(KeyCode, Key, bevy::input::ButtonState)]| {
        for (key_code, logical_key, state) in keys.iter().cloned() {
            let window = Entity::PLACEHOLDER;
            app.world_mut().write_message(KeyboardInput {
                key_code,
                logical_key,
                state,
                text: None,
                repeat: false,
                window,
            });
        }
        app.update();
        let enter = app.world().resource::<ButtonInput<KeyCode>>().just_pressed(KeyCode::Enter);
        let typed = app.world().resource::<Field>().0.contains(&KeyCode::Enter);
        (app.world().resource::<AltEnter>().pressed, enter, typed)
    };
    let chord = [
        (KeyCode::AltLeft, Key::Alt, Pressed),
        (KeyCode::Enter, Key::Enter, Pressed),
        (KeyCode::Enter, Key::Enter, Released),
        (KeyCode::AltLeft, Key::Alt, Released),
    ];
    assert_eq!(send(&mut app, &chord), (true, false, false));
    assert_eq!(send(&mut app, &[(KeyCode::Enter, Key::Enter, Pressed)]), (false, true, true));
}

#[test]
fn the_movies_display_follows_the_window_once_it_settles() {
    let class = |w, h| DisplayClass::of_window(w, h);
    let (wide, sd) = (DisplayClass::WIDESCREEN, DisplayClass { widescreen: false });
    for (w, h, want) in [(1024, 768, sd), (1280, 800, wide), (1920, 1080, wide), (2560, 1080, wide), (5120, 1440, wide)]
    {
        let mut pending = None;
        let told = if want == wide { sd } else { wide };
        assert_eq!(display_due(&mut pending, class(w, h), told, false, 0.0), Some(want), "{w} x {h}");
        assert_eq!(display_due(&mut None, class(w, h), want, true, 0.0), None, "{w} x {h}");
    }
    let mut pending = None;
    assert_eq!(display_due(&mut pending, sd, wide, true, 10.0), None);
    assert_eq!(display_due(&mut pending, sd, wide, true, 10.4), None);
    assert_eq!(display_due(&mut pending, wide, wide, true, 10.45), None);
    assert_eq!(pending, None);
    assert_eq!(display_due(&mut pending, sd, wide, true, 10.6), None);
    assert_eq!(display_due(&mut pending, sd, wide, true, 11.0), None);
    assert_eq!(display_due(&mut pending, sd, wide, true, 11.1), Some(sd));
}
