//! Read-only native seam probe. Does not inject input or transfer files.
//! `target X Y`, or `source X Y SECONDS` while manually dragging a test file.
use anyhow::{Context, Result};
use rshare_platform::folder_drop;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).context("target X Y | source X Y SECONDS")?;
    #[cfg(target_os = "macos")]
    if mode == "windows" {
        use core_foundation::{
            base::{CFType, TCFType},
            dictionary::CFDictionary,
            string::CFString,
        };
        use core_graphics::window::{
            copy_window_info, kCGNullWindowID, kCGWindowListOptionOnScreenOnly,
        };
        for value in copy_window_info(kCGWindowListOptionOnScreenOnly, kCGNullWindowID)
            .context("window list")?
            .iter()
        {
            let info: CFDictionary<CFString, CFType> =
                unsafe { CFDictionary::wrap_under_get_rule((*value).cast()) };
            let name = info
                .find(CFString::new("kCGWindowOwnerName"))
                .and_then(|v| v.downcast::<CFString>())
                .map(|v| v.to_string());
            if matches!(name.as_deref(), Some("Finder" | "访达")) {
                println!("{info:?}");
            }
        }
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if mode == "pointer" {
        use core_graphics::{
            event::CGEvent,
            event_source::{CGEventSource, CGEventSourceStateID},
        };
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .map_err(|_| anyhow::anyhow!("event source"))?;
        let point = CGEvent::new(source)
            .map_err(|_| anyhow::anyhow!("pointer"))?
            .location();
        println!("point={},{}", point.x, point.y);
        println!(
            "{}",
            folder_drop::destination_folder(point.x as i32, point.y as i32)
                .await?
                .display()
        );
        return Ok(());
    }
    let x = args.get(2).context("X")?.parse()?;
    let y = args.get(3).context("Y")?.parse()?;
    match mode.as_str() {
        "target" => println!("{}", folder_drop::destination_folder(x, y).await?.display()),
        "source" => {
            let origin = folder_drop::begin_drag(x, y).await?;
            println!("ready {origin:?}");
            let seconds: u64 = args.get(4).context("SECONDS")?.parse()?;
            let until =
                tokio::time::Instant::now() + std::time::Duration::from_secs(seconds.min(60));
            let mut found = false;
            while tokio::time::Instant::now() < until {
                let paths = folder_drop::dragged_files(origin.clone()).await?;
                if !paths.is_empty() {
                    println!("{}", serde_json::to_string(&paths)?);
                    found = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            if !found {
                println!("no new file drag");
            }
        }
        _ => anyhow::bail!("target X Y | source X Y SECONDS"),
    }
    Ok(())
}
