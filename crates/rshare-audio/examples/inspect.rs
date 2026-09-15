//! Read-only native endpoint inspection; no daemon restart or capture.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let status = rshare_audio::backend::status();
    println!("backend: {}\nloaded: {}\nerror: {:?}", status.name, status.ready, status.error);
    let endpoints = rshare_audio::backend::enumerate().map_err(std::io::Error::other)?;
    println!("{}", serde_json::to_string_pretty(&endpoints)?);
    Ok(())
}
