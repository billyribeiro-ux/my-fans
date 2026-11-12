//! The only image decoder entry point. Invoked inside the OS sandbox by the
//! worker; no platform state, database, provider configuration or runtime loads.
use image::{ImageReader, Limits, codecs::jpeg::JpegEncoder, imageops::FilterType};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
fn decode(input: &Path, full: &Path, preview: &Path) -> Result<(), &'static str> {
    let file = fs::File::open(input).map_err(|_| "input")?;
    if file.metadata().map_err(|_| "input")?.len() > 10 * 1024 * 1024 {
        return Err("image_size");
    };
    let mut bytes = Vec::new();
    file.take(10 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "input")?;
    if bytes.len() > 10 * 1024 * 1024 {
        return Err("image_size");
    };
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "image_format")?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(12000);
    limits.max_image_height = Some(12000);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| "image_limits")?;
    let large = image.resize(4096, 4096, FilterType::Lanczos3).to_rgb8();
    let small = image
        .resize(64, 64, FilterType::Triangle)
        .blur(8.0)
        .to_rgb8();
    let full = fs::File::create(full).map_err(|_| "output")?;
    let preview = fs::File::create(preview).map_err(|_| "output")?;
    JpegEncoder::new_with_quality(full, 88)
        .encode_image(&large)
        .map_err(|_| "image_encode")?;
    JpegEncoder::new_with_quality(preview, 70)
        .encode_image(&small)
        .map_err(|_| "preview_encode")?;
    Ok(())
}
fn self_test(canary: &Path) -> Result<(), &'static str> {
    let vars: Vec<_> = std::env::vars().map(|(key, _)| key).collect();
    if vars.len() != 3
        || !["PATH", "LANG", "TZ"]
            .iter()
            .all(|key| vars.iter().any(|v| v == key))
    {
        return Err("environment");
    };
    // The canary belongs to this UID and exists outside scratch. An ordinary
    // non-root process could read it, so EACCES proves the filesystem boundary.
    if !matches!(fs::read(canary),Err(e) if e.kind()==std::io::ErrorKind::PermissionDenied) {
        return Err("secret_read");
    };
    if !matches!(fs::OpenOptions::new().write(true).truncate(true).open(canary),Err(e) if e.kind()==std::io::ErrorKind::PermissionDenied)
    {
        return Err("secret_truncate");
    };
    if !matches!(std::net::TcpListener::bind("127.0.0.1:0"),Err(e) if e.kind()==std::io::ErrorKind::PermissionDenied)
    {
        return Err("network");
    };
    #[cfg(target_os = "linux")]
    if !matches!(std::os::unix::net::UnixStream::pair(),Err(e) if e.kind()==std::io::ErrorKind::PermissionDenied)
    {
        return Err("unix_socket");
    };
    fs::File::create("probe")
        .and_then(|mut f| f.write_all(b"scratch allowed"))
        .map_err(|_| "scratch_write")?;
    if fs::read("probe").map_err(|_| "scratch_read")? != b"scratch allowed" {
        return Err("scratch_contents");
    };
    fs::remove_file("probe").map_err(|_| "scratch_remove")?;
    println!("media_sandbox_verified");
    Ok(())
}
fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().collect();
    let outcome = match args.as_slice() {
        [_, mode, input, full, preview] if mode == "image" => {
            decode(Path::new(input), Path::new(full), Path::new(preview))
        }
        [_, mode, canary] if mode == "self-test" => self_test(Path::new(canary)),
        _ => Err("arguments"),
    };
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("media_processing_failed:{code}");
            std::process::ExitCode::from(1)
        }
    }
}
