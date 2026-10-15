//! Record real Ferrofluid software-renderer animation frames (no model/backend).
//! cargo run -p alloyos-ferrofluid --example readme_capture -- boot /tmp/alloyos-readme-capture/boot
//! cargo run -p alloyos-ferrofluid --example readme_capture -- keyboard /tmp/alloyos-readme-capture/keyboard
use alloyos_ferrofluid::system_ui::FerrofluidSystemUi;
use alloyos_proto::{
    display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH},
    InputEvent, InputEventKind,
};
use std::{fs, io::BufWriter, path::PathBuf, time::Duration};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let scene = args.next().ok_or("expected boot, keyboard, setup, memory, weather, or voice")?;
    if scene != "boot" && scene != "keyboard" && scene != "setup" && scene != "memory" && scene != "weather" && scene != "voice" {
        return Err("expected boot, keyboard, setup, memory, weather, or voice".into());
    }
    let output = PathBuf::from(args.next().ok_or("expected output directory")?);
    fs::create_dir_all(&output)?;
    let width = PHONE_WIDTH;
    let height = PHONE_HEIGHT;
    let mut ui = FerrofluidSystemUi::new(width, height, PHONE_SCALE as f32)
        .map_err(|err| format!("System UI: {err:?}"))?;
    ui.set_booting(scene == "boot");
    let weather = if scene == "weather" {
        let data: serde_json::Value = serde_json::from_slice(&fs::read(args.next().ok_or("expected weather story JSON")?)?)?;
        ui.open_keyboard();
        Some(data)
    } else { None };
    let memory = if scene == "memory" {
        let data: serde_json::Value = serde_json::from_slice(&fs::read(args.next().ok_or("expected memory story JSON")?)?)?;
        let tile: alloyos_mcp::AppTile = serde_json::from_value(data["active"].clone())?;
        ui.set_chat_items(&[alloyos_ferrofluid::ChatItem::Tile(tile)]);
        Some(data)
    } else { None };
    if scene == "keyboard" {
        if let Some(path) = args.next() {
            let story: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
            ui.set_transcript(
                story["scenes"][0]["items"][0]["text"]
                    .as_str()
                    .ok_or("missing greeting")?,
            );
        }
    }
    let fps = 20u64;
    let story = if scene == "setup" {
        Some(serde_json::from_slice::<serde_json::Value>(&fs::read(
            args.next().ok_or("expected setup story JSON path")?,
        )?)?)
    } else {
        None
    };
    let mut timeline = Vec::new();
    let mut paused = false;
    if let Some(story) = &story {
        let mut at = 0;
        for entry in story["scenes"].as_array().ok_or("missing scenes")? {
            timeline.push((at, entry));
            at += entry["hold_ms"].as_u64().unwrap_or(2200).max(3200) * fps / 1000;
        }
    }
    let frames = if scene == "boot" {
        100
    } else if scene == "setup" {
        timeline
            .last()
            .map(|(at, entry)| {
                at + entry["hold_ms"].as_u64().unwrap_or(2200).max(3200) * fps / 1000
            })
            .unwrap_or(0)
    } else if scene == "memory" || scene == "weather" || scene == "voice" {
        240
    } else {
        140
    };
    // Move the oversized knowledge tile to its heading before recording.
    // These are the same real scroll events used during the visible take.
    let memory_preroll_ms = if scene == "memory" {
        for step in 0..40u64 {
            if step==5 { ui.handle_input(InputEvent{kind:InputEventKind::PointerButtonDown,code:0x110,x:600,y:400}); }
            if step>5 && step<30 { ui.handle_input(InputEvent{kind:InputEventKind::PointerMove,code:0x110,x:600,y:400+(step-5) as i32*80}); }
            if step==30 { ui.handle_input(InputEvent{kind:InputEventKind::PointerButtonUp,code:0x110,x:600,y:2400}); }
            ui.render(Duration::from_millis(step*50));
        }
        2000
    } else {0};
    let mut voice_started = false;
    let mut weather_submitted = false;
    let mut weather_pinned = false;
    for frame in 0..frames {
        if scene == "voice" {
            use alloyos_mcp::display::VoiceState;
            if frame==20 {
                for kind in [InputEventKind::PointerButtonDown,InputEventKind::PointerButtonUp] {ui.handle_input(InputEvent{kind,code:0x110,x:459,y:2454});}
                voice_started=ui.take_voice_toggle();
                assert!(voice_started,"actual mic tap must request voice");
                ui.set_voice_state(VoiceState::Connecting,"","");
            }
            if frame==35 {ui.set_voice_state(VoiceState::Listening,"","");}
            if frame==55 {ui.set_voice_state(VoiceState::Listening,"Can you help","");}
            if frame==65 {ui.set_voice_state(VoiceState::Listening,"Can you help me plan my day?","");}
            if frame==90 {
                ui.set_voice_state(VoiceState::Responding,"Of course.","");
            }
            if frame==105 {
                ui.set_voice_state(VoiceState::Responding,"Of course. What would you like to get done?","");
            }
            if frame==145 {
                ui.set_chat_items(&[alloyos_ferrofluid::ChatItem::Text("You: Can you help me plan my day?".into()),alloyos_ferrofluid::ChatItem::Text("Welder: Of course. What would you like to get done?".into())]);
                ui.set_voice_state(VoiceState::Listening,"","");
            }
            if frame==160 {ui.set_voice_state(VoiceState::Muted,"","");}
            if frame==210 {ui.set_voice_state(VoiceState::Off,"","");}
            let level=if (35..90).contains(&frame) || (90..145).contains(&frame) {
                let t=frame as f32;
                (150.0+540.0*(t*0.43).sin().abs()+180.0*(t*0.19).sin().abs()) as u16
            } else {0};
            ui.set_voice_level(level);
        }

        if scene == "weather" {
            let request = "Weather in San Francisco?";
            if frame >= 15 && frame < 15 + request.len() as u64 {
                let c=request.as_bytes()[(frame-15) as usize] as char;
                let lower=c.to_ascii_lowercase();
                let code=match lower { 'w'=>17,'e'=>18,'a'=>30,'t'=>20,'h'=>35,'r'=>19,' '=>57,'i'=>23,'n'=>49,'s'=>31,'f'=>33,'c'=>46,'o'=>24,'?'=>53,_=>unreachable!() };
                let shifted=c.is_uppercase() || c=='?';
                if shifted {ui.handle_input(InputEvent{kind:InputEventKind::KeyDown,code:42,x:0,y:0});}
                for kind in [InputEventKind::KeyDown,InputEventKind::KeyUp] {ui.handle_input(InputEvent{kind,code,x:0,y:0});}
                if shifted {ui.handle_input(InputEvent{kind:InputEventKind::KeyUp,code:42,x:0,y:0});}
            }
            if frame==60 {
                for kind in [InputEventKind::KeyDown,InputEventKind::KeyUp] {ui.handle_input(InputEvent{kind,code:28,x:0,y:0});}
                let submitted=ui.take_submission().ok_or("weather request did not submit")?;
                assert_eq!(submitted,request);
                weather_submitted=true;
                ui.set_chat_items(&[alloyos_ferrofluid::ChatItem::Text(format!("You: {request}"))]);
                ui.set_reply_pending(true);
            }
            if frame==85 {
                let tile:alloyos_mcp::AppTile=serde_json::from_value(weather.as_ref().unwrap()["tile"].clone())?;
                ui.set_reply_pending(false);
                ui.set_chat_items(&[alloyos_ferrofluid::ChatItem::Text(format!("You: {request}")),alloyos_ferrofluid::ChatItem::Tile(tile)]);
            }
            if frame==130 {
                for kind in [InputEventKind::PointerButtonDown,InputEventKind::PointerButtonUp] {ui.handle_input(InputEvent{kind,code:0x110,x:1025,y:485});}
                weather_pinned=ui.dashboard_state().pinned_ids().iter().any(|id|id=="weather.card");
                eprintln!("Weather actual pin callback verified: {weather_pinned}");
                assert!(weather_pinned, "weather pin pointer failed to activate actual tile pin");
            }
            if frame==165 {ui.handle_input(InputEvent{kind:InputEventKind::PointerButtonDown,code:0x110,x:5,y:1200});}
            if frame>165 && frame<190 {ui.handle_input(InputEvent{kind:InputEventKind::PointerMove,code:0x110,x:5+(frame-165) as i32*39,y:1200});}
            if frame==190 {ui.handle_input(InputEvent{kind:InputEventKind::PointerButtonUp,code:0x110,x:980,y:1200});}
        }
        if scene == "memory" {
            // Hold the heading, scroll to the final controls, open the actual
            // keyboard button, then activate Pause Learning with real input.
            let start=50;
            if frame==start { ui.handle_input(InputEvent{kind:InputEventKind::PointerButtonDown,code:0x110,x:600,y:1500}); }
            if frame>start && frame<start+25 {ui.handle_input(InputEvent{kind:InputEventKind::PointerMove,code:0x110,x:600,y:1500-(frame-start) as i32*60});}
            if frame==start+25 {ui.handle_input(InputEvent{kind:InputEventKind::PointerButtonUp,code:0x110,x:600,y:0});}
            if frame==110 {
                for kind in [InputEventKind::PointerButtonDown,InputEventKind::PointerButtonUp] {ui.handle_input(InputEvent{kind,code:0x110,x:760,y:2470});}
            }
            if frame==165 {
                for kind in [InputEventKind::PointerButtonDown,InputEventKind::PointerButtonUp] {ui.handle_input(InputEvent{kind,code:0x110,x:600,y:1600});}
            }
            if let Some(action) = ui.take_tile_action() {
                eprintln!("Memory action reached: {}", action.id);
                if action.id == "pause_learning" {
                    let tile: alloyos_mcp::AppTile=serde_json::from_value(memory.as_ref().unwrap()["paused"].clone())?;
                    ui.set_chat_items(&[alloyos_ferrofluid::ChatItem::Tile(tile)]);
                    paused=true;
                }
            }
        }
        for (at, entry) in &timeline {
            if frame == *at {
                let text = entry["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        if item["role"] == "user" {
                            format!("> {}", item["text"].as_str().unwrap())
                        } else {
                            item["text"].as_str().unwrap().to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                ui.set_transcript(&text);
            }
        }
        if scene == "boot" && frame == 70 {
            ui.set_booting(false);
        }
        if scene == "keyboard" {
            if frame == 20 {
                ui.open_keyboard();
            }
            // Actual pointer/key input; type a draft without generating a response.
            let codes = [35, 18, 38, 38, 24]; // Linux keycodes: hello
            if frame >= 45 && frame < 70 && (frame - 45) % 5 == 0 {
                let code = codes[((frame - 45) / 5) as usize];
                for kind in [InputEventKind::KeyDown, InputEventKind::KeyUp] {
                    ui.handle_input(InputEvent {
                        kind,
                        code,
                        x: 0,
                        y: 0,
                    });
                }
            }
            // Hold two genuine soft keys to expose their expanded feedback.
            for (down, up, x, y) in [(75, 82, 90, 2080), (88, 95, 420, 2080)] {
                if frame == down || frame == up {
                    ui.handle_input(InputEvent {
                        kind: if frame == down {
                            InputEventKind::PointerButtonDown
                        } else {
                            InputEventKind::PointerButtonUp
                        },
                        code: 0x110,
                        x,
                        y,
                    });
                }
            }
            if frame == 110 {
                for kind in [
                    InputEventKind::PointerButtonDown,
                    InputEventKind::PointerButtonUp,
                ] {
                    ui.handle_input(InputEvent {
                        kind,
                        code: 0x110,
                        x: 600,
                        y: 800,
                    });
                }
            }
        }
        ui.render(Duration::from_millis(memory_preroll_ms + frame * 1000 / fps));
        if std::env::var_os("ALLOYOS_CAPTURE_SNAPSHOTS").is_some()
            && ![0,30,55,75,100,125,140,160,190,200,239].contains(&frame) { continue; }
        let file = BufWriter::new(fs::File::create(
            output.join(format!("frame-{frame:04}.png")),
        )?);
        let mut bytes = Vec::with_capacity((PHONE_WIDTH * PHONE_HEIGHT * 3) as usize);
        for pixel in ui.pixels() {
            bytes.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8]);
        }
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fastest);
        encoder.write_header()?.write_image_data(&bytes)?;
    }
    if scene == "voice" {assert!(voice_started);eprintln!("Simulated voice: actual mic callback verified; no microphone, network, or model");}
    if scene == "weather" { eprintln!("Weather submission verified: {weather_submitted}; pin verified: {weather_pinned}"); }
    if scene == "memory" { eprintln!("Pause callback verified: {paused}"); assert!(paused,"memory pause pointer must reach actual callback"); }
    println!(
        "Recorded {scene}: {frames} real renderer frames at {fps} fps in {}",
        output.display()
    );
    Ok(())
}
