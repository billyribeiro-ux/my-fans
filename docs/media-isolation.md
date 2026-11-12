# Media processing and isolation

The worker copies quarantined inputs into a private `0700` scratch directory. It invokes the separate `myfans-media` image decoder or FFmpeg through `myfans-media-sandbox`. Decoder processes do not initialize database connections or provider clients. The Rust supervisor clears their environment, bounds output and execution time, and kills the child when a lease is lost or its future is cancelled. Scratch directories use a drop guard; Linux also kills an orphaned decoder when its supervisor dies.

The Linux launcher requires Landlock ABI 3 or newer, an unprivileged UID and no effective, permitted or inheritable capabilities. It enables `no_new_privs`, closes inherited descriptors other than the three explicit standard streams, and passes only `PATH`, `LANG` and `TZ`. Failure to enforce any boundary exits125 and leaves processing retryable; there is no unrestricted decoder fallback.

Landlock grants read/write access to flat files in that one scratch directory, read access to library directories and exact execute rights to the selected decoder and the installed musl/glibc loader. Application secrets, `/proc` process contents, other scratch directories, uploaded executables and application configuration have no read grant. ABI3 is required for truncation protection. See the [Linux Landlock documentation](https://docs.kernel.org/userspace-api/landlock.html).

Seccomp rejects sockets, network operations, cross-process memory/inspection, namespace/mount changes, io_uring, privileged kernel facilities and process creation. Only thread creation is allowed. It validates the native syscall architecture and rejects the x32 alternate ABI. The filter and filesystem restrictions are inherited by decoder threads. See the [Linux seccomp documentation](https://docs.kernel.org/userspace-api/seccomp_filter.html).

| Bound                                | Value                                                                        |
| ------------------------------------ | ---------------------------------------------------------------------------- |
| Source image                         | 10MiB; at most12000 pixels per dimension;128MiB decoder allocation           |
| Source video                         | 256MiB;5minutes;4096 pixels per dimension;16 streams                         |
| Decoder address space / CPU          | 1GiB /300CPU seconds                                                         |
| Video pipeline wall time             | 300seconds across probe, encode, remux and preview                           |
| Individual output / open descriptors | 512MiB /64                                                                   |
| Launcher arguments                   | 256 arguments;32KiB total including terminating NUL bytes                    |
| Threads per real UID                 | 256; this Linux limit includes other processes with the same UID on the host |
| Supervisor output monitoring         | 64KiB stdout;400 scratch entries;768MiB aggregate scratch checked every250ms |
| HLS segment                          | 10MiB maximum;2second target;at most151 segments per rendition               |

The aggregate scratch monitor is a rejection threshold, not an atomic filesystem quota. Container memory, CPU and writable-volume size provide additional boundaries. Private Compose and the Linux smoke set a cgroup PID limit of256; this is distinct from the launcher's host-UID-wide `RLIMIT_NPROC=256`. The ECS task definition does not expose that Docker PID setting. Worker placement uses isolated capacity and separates replicas across hosts. Do not share a worker UID with unrelated host workloads; UID-wide resource exhaustion fails processing closed.

Video encoding produces bounded720p/360p H.264/AAC renditions without upscaling, two-second aligned keyframes, a private MP4 fallback and a blurred JPEG preview. Playlists contain only validated relative UUID filenames; URI directives, unexpected tags, missing end markers and nonsequential segments are rejected. Segments are stored before playlists and the database transition is atomic. Every playlist and segment request rechecks authentication, entitlement and moderation. Paid access survives contact/discovery blocks until refunded or revoked, matching the explicit platform policy. See the [FFmpeg HLS muxer documentation](https://ffmpeg.org/ffmpeg-formats.html#hls).

macOS development uses `sandbox-exec` with a deny-default profile and the same cleared environment and Rust supervision. It is not evidence for the Linux resource/seccomp boundary. Production startup and `doctor` run an actual confinement self-test; kernel version alone is insufficient. Unsupported kernels fail startup.

Run `bash scripts/verify-media-isolation.sh sha256:IMMUTABLE_IMAGE_ID` after building each Linux release. It checks same-UID filesystem read/truncate denial, network denial and environment removal, performs real isolated JPEG processing, and rejects unsafe executables/workspaces and excessive argument count/bytes. It runs under both dedicated UID 65532 and actual application UID 1000 with container PID limit of 256.

The smoke invokes `MEDIA_ROOT=/tmp myfans-api verify-media-pipeline` inside its private container. This database-free diagnostic generates fixed synthetic videos with and without audio, then calls the production renderer: both HLS tiers, AAC where present, MP4 fallback, blurred JPEG and validated playlists/segments. Scratch guards remove each fixture after use. The October 8 role review found that the former 128-argument launcher limit rejected the actual 133-argument adaptive encode; the bounded 256-argument / 32 KiB limit and this full-pipeline check address that gap. Poster filtering also explicitly uses one filter thread. No filesystem, syscall or other resource boundary is relaxed.

Also run the full application upload/browser suite under its actual runtime UID. The [current release evidence](verification.md) records exact image digests and observed results; a short standalone FFmpeg invocation does not establish that the entire application pipeline works.
