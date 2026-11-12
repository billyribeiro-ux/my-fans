use super::*;
use std::{path::Path, time::Instant};
const MAX_DURATION: f64 = 300.0;
const MAX_SEGMENTS: usize = 151;
const MAX_SEGMENT_BYTES: u64 = 10 * 1024 * 1024;
struct Rendered {
    full: String,
    preview: String,
    size: i64,
    objects: Vec<(String, &'static str)>,
}
async fn run(
    work: &Path,
    binary: &str,
    args: Vec<String>,
    started: Instant,
    limit: u64,
) -> Result<Vec<u8>> {
    let remaining = 300u64.saturating_sub(started.elapsed().as_secs());
    if remaining == 0 {
        return Err(bad("Video processing exceeded its time limit."));
    }
    media_processor::execute(work, binary, &args, remaining.min(limit)).await
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).into()).collect()
}
/// Release diagnostic using the production renderer, with and without audio.
/// Inputs are fixed synthetic fixtures; all output follows the scratch guard.
pub async fn verify_pipeline(root: &Path) -> Result<()> {
    for audio in [true, false] {
        let work = workdir::WorkDir::create(root)?;
        let id = Uuid::new_v4();
        let mut source = args(&[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-filter_threads",
            "1",
            "-filter_complex_threads",
            "1",
            "-threads",
            "2",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=1280x720:r=30",
        ]);
        if audio {
            source.extend(args(&[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-c:a",
                "aac",
            ]));
        }
        source.extend(args(&[
            "-t",
            "4",
            "-threads",
            "2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "source.mp4",
        ]));
        media_processor::execute(work.path(), "ffmpeg", &source, 30).await?;
        let rendered = render(work.path(), id, &work.path().join("source.mp4")).await?;
        let master = bounded_manifest(&work.path().join(format!("{id}.m3u8"))).await?;
        if master.contains("mp4a.40.2") != audio || rendered.objects.len() < 9 {
            return Err(bad("Media pipeline verification failed."));
        }
    }
    Ok(())
}
pub async fn process_video(s: &PlatformState, id: Uuid, path: &Path) -> Result<()> {
    let work = path.parent().ok_or(bad("Invalid video workspace."))?;
    match render(work, id, path).await {
        Ok(rendered) => {
            // Segments precede playlists; the single DB transition exposes none
            // of them until all puts complete and moderation later approves.
            for (key, content_type) in &rendered.objects {
                s.storage
                    .put_file(key, &work.join(key), content_type)
                    .await?;
            }
            sqlx::query("UPDATE media_assets SET state='quarantine',storage_key=$2,preview_key=$3,content_type='video/mp4',size_bytes=$4,hls_ready=true WHERE id=$1 AND state='processing'").bind(id).bind(rendered.full).bind(rendered.preview).bind(rendered.size).execute(&s.db).await?;
            Ok(())
        }
        Err(error) => {
            if error.0.is_server_error() {
                return Err(error);
            }
            sqlx::query("UPDATE media_assets SET state='rejected',rejection_reason=$2 WHERE id=$1 AND state='processing'").bind(id).bind(error.1).execute(&s.db).await?;
            Ok(())
        }
    }
}
async fn render(work: &Path, id: Uuid, path: &Path) -> Result<Rendered> {
    let started = Instant::now();
    let path = tokio::fs::canonicalize(path).await.map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Video scratch storage failed.",
        )
    })?;
    let mut probe = args(&[
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe",
        "-show_entries",
        "stream=codec_type,width,height:format=duration",
        "-of",
        "json",
    ]);
    probe.push(path.to_string_lossy().into_owned());
    let bytes = run(work, "ffprobe", probe, started, 10).await?;
    let metadata: Value =
        serde_json::from_slice(&bytes).map_err(|_| bad("Invalid video metadata."))?;
    let streams = metadata["streams"]
        .as_array()
        .filter(|v| v.len() <= 16)
        .ok_or(bad("Invalid video streams."))?;
    let video = streams
        .iter()
        .find(|v| v["codec_type"] == "video")
        .ok_or(bad("A video stream is required."))?;
    let audio = streams.iter().any(|v| v["codec_type"] == "audio");
    let duration = metadata["format"]["duration"]
        .as_str()
        .and_then(|v| v.parse::<f64>().ok())
        .ok_or(bad("The video duration is invalid."))?;
    let width = video["width"].as_u64().unwrap_or(0);
    let height = video["height"].as_u64().unwrap_or(0);
    if !duration.is_finite()
        || !(0.1..=MAX_DURATION).contains(&duration)
        || width == 0
        || height == 0
        || width > 4096
        || height > 4096
    {
        return Err(bad(
            "Videos must be up to five minutes and 4096 pixels per dimension.",
        ));
    }
    let full = format!("{id}.mp4");
    let preview = format!("{id}.preview.jpg");
    let low_list = format!("{id}.360p.m3u8");
    let high_list = format!("{id}.720p.m3u8");
    let master = format!("{id}.m3u8");
    // Scale once per tier, no upscaling, fixed two-second GOPs and predictable
    // bitrate bounds. Both tiers encode in one isolated invocation.
    let mut encode = args(&[
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-protocol_whitelist",
        "file,pipe",
        "-threads",
        "2",
        "-i",
    ]);
    encode.push(path.to_string_lossy().into_owned());
    encode.extend(args(&["-filter_complex_threads","2","-filter_complex","[0:v:0]split=2[hi][lo];[hi]scale=w='min(1280,iw)':h='min(720,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1[high];[lo]scale=w='min(640,iw)':h='min(360,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1[low]","-map","[high]","-map","0:a:0?","-map_metadata","-1","-map_chapters","-1"]));
    encode.extend(codec("2200k", "2600k", "5200k", "3.1"));
    encode.extend(args(&[
        "-movflags",
        "+faststart",
        "-t",
        "300",
        "-fs",
        "536870912",
    ]));
    encode.push(full.clone());
    encode.extend(args(&[
        "-map",
        "[low]",
        "-map",
        "0:a:0?",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
    ]));
    encode.extend(codec("600k", "800k", "1600k", "3.0"));
    encode.extend(args(&[
        "-t",
        "300",
        "-f",
        "hls",
        "-hls_time",
        "2",
        "-hls_list_size",
        "0",
        "-hls_playlist_type",
        "vod",
        "-hls_flags",
        "independent_segments+temp_file",
        "-start_number",
        "0",
        "-hls_segment_filename",
    ]));
    encode.push(format!("{id}.360p-%05d.ts"));
    encode.push(low_list.clone());
    run(work, "ffmpeg", encode, started, 290).await?;
    let size = file_size(&work.join(&full), 512 * 1024 * 1024).await?;
    let mut high = args(&[
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]);
    high.push(full.clone());
    high.extend(args(&[
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
        "-c",
        "copy",
        "-f",
        "hls",
        "-hls_time",
        "2",
        "-hls_list_size",
        "0",
        "-hls_playlist_type",
        "vod",
        "-hls_flags",
        "independent_segments+temp_file",
        "-start_number",
        "0",
        "-hls_segment_filename",
    ]));
    high.push(format!("{id}.720p-%05d.ts"));
    high.push(high_list.clone());
    run(work, "ffmpeg", high, started, 30).await?;
    let mut poster = args(&[
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-protocol_whitelist",
        "file,pipe",
        "-threads",
        "2",
        "-i",
    ]);
    poster.push(full.clone());
    poster.extend(args(&[
        "-frames:v",
        "1",
        "-filter_threads",
        "1",
        "-vf",
        "scale=64:64:force_original_aspect_ratio=decrease,gblur=sigma=8",
        "-threads",
        "2",
        "-update",
        "1",
    ]));
    poster.push(preview.clone());
    run(work, "ffmpeg", poster, started, 15).await?;
    file_size(&work.join(&preview), 1024 * 1024).await?;
    let mut objects = Vec::new();
    for (tier, list) in [("720p", &high_list), ("360p", &low_list)] {
        let text = bounded_manifest(&work.join(list)).await?;
        for key in validate_playlist(&text, id, tier)? {
            file_size(&work.join(&key), MAX_SEGMENT_BYTES).await?;
            objects.push((key, "video/mp2t"));
        }
    }
    let audio_codec = if audio { ",mp4a.40.2" } else { "" };
    let text = format!(
        "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-STREAM-INF:BANDWIDTH=2900000,CODECS=\"avc1.64001f{audio_codec}\"\n{high_list}\n#EXT-X-STREAM-INF:BANDWIDTH=1100000,CODECS=\"avc1.64001e{audio_codec}\"\n{low_list}\n"
    );
    tokio::fs::write(work.join(&master), text)
        .await
        .map_err(|_| {
            Error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Video manifest storage failed.",
            )
        })?;
    objects.extend([
        (full.clone(), "video/mp4"),
        (preview.clone(), "image/jpeg"),
        (high_list, "application/vnd.apple.mpegurl"),
        (low_list, "application/vnd.apple.mpegurl"),
        (master, "application/vnd.apple.mpegurl"),
    ]);
    Ok(Rendered {
        full,
        preview,
        size: size as i64,
        objects,
    })
}
fn codec(bitrate: &str, maxrate: &str, buffer: &str, level: &str) -> Vec<String> {
    let mut values = args(&[
        "-c:v",
        "libx264",
        "-threads",
        "2",
        "-preset",
        "veryfast",
        "-profile:v",
        "high",
        "-level:v",
    ]);
    values.push(level.into());
    values.extend(args(&[
        "-pix_fmt",
        "yuv420p",
        "-r",
        "30",
        "-fps_mode",
        "cfr",
        "-g",
        "60",
        "-keyint_min",
        "60",
        "-sc_threshold",
        "0",
        "-force_key_frames",
        "expr:gte(t,n_forced*2)",
        "-b:v",
    ]));
    values.push(bitrate.into());
    values.push("-maxrate".into());
    values.push(maxrate.into());
    values.push("-bufsize".into());
    values.push(buffer.into());
    values.extend(args(&[
        "-c:a", "aac", "-b:a", "128k", "-ar", "48000", "-ac", "2",
    ]));
    values
}
async fn file_size(path: &Path, max: u64) -> Result<u64> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Video output is unavailable.",
        )
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() >= max {
        return Err(bad("Video output exceeded processing limits."));
    }
    Ok(metadata.len())
}
async fn bounded_manifest(path: &Path) -> Result<String> {
    use tokio::io::AsyncReadExt;
    file_size(path, 64 * 1024).await?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| bad("Invalid video playlist."))?;
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| bad("Invalid video playlist."))?;
    if bytes.len() > 64 * 1024 {
        return Err(bad("Invalid video playlist."));
    }
    String::from_utf8(bytes).map_err(|_| bad("Invalid video playlist."))
}
fn validate_playlist(text: &str, id: Uuid, tier: &str) -> Result<Vec<String>> {
    if !text.starts_with("#EXTM3U\n")
        || !text.trim_end().ends_with("#EXT-X-ENDLIST")
        || text.contains('\r')
    {
        return Err(bad("Invalid video playlist."));
    }
    let mut keys = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') {
            if !(matches!(
                line,
                "#EXTM3U"
                    | "#EXT-X-INDEPENDENT-SEGMENTS"
                    | "#EXT-X-PLAYLIST-TYPE:VOD"
                    | "#EXT-X-ENDLIST"
            ) || [
                "#EXT-X-VERSION:",
                "#EXT-X-TARGETDURATION:",
                "#EXT-X-MEDIA-SEQUENCE:",
                "#EXTINF:",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix)))
                || line.contains("URI=")
            {
                return Err(bad("Invalid video playlist."));
            }
        } else if !line.is_empty() {
            if keys.len() >= MAX_SEGMENTS || line != format!("{id}.{tier}-{:05}.ts", keys.len()) {
                return Err(bad("Invalid video segment reference."));
            }
            keys.push(line.to_owned());
        }
    }
    if keys.is_empty() {
        return Err(bad("Video playlist has no segments."));
    }
    Ok(keys)
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn playlist_references_are_sequential_local_and_bounded() {
        let id = Uuid::nil();
        let good = format!(
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXTINF:2.000000,\n{id}.720p-00000.ts\n#EXT-X-ENDLIST\n"
        );
        assert_eq!(validate_playlist(&good, id, "720p").unwrap().len(), 1);
        for changed in [
            good.replace(
                &format!("{id}.720p-00000.ts"),
                "https://169.254.170.2/secret",
            ),
            good.replace("00000.ts", "00001.ts"),
            good.replace(
                "#EXTINF:2.000000,",
                "#EXT-X-KEY:METHOD=AES-128,URI=\"/run/secrets/password\"",
            ),
            good.replace("#EXT-X-ENDLIST", ""),
        ] {
            assert!(validate_playlist(&changed, id, "720p").is_err());
        }
    }
}
