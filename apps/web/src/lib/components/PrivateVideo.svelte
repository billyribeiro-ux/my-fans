<script lang="ts">
  import type Hls from 'hls.js';
  import { privateHlsMediaId, privateHlsUrl } from '#lib/private-media.ts';
  let {
    src,
    hlsUrl,
    poster,
    captionsUrl,
    label,
  }: {
    src: string;
    hlsUrl?: string | null;
    poster?: string | null;
    captionsUrl?: string | null;
    label: string;
  } = $props();
  let error = $state('');
  let fallbackNotice = $state('');
  let retry = $state(0);

  function playback(mp4: string, adaptive: string | null | undefined, attempt: number) {
    void attempt;
    return (node: HTMLVideoElement) => {
      let disposed = false;
      let player: Hls | undefined;
      let usingAdaptive = false;
      let restorePosition: (() => void) | undefined;
      error = '';
      fallbackNotice = '';
      function useMp4() {
        if (disposed) return;
        const position = Number.isFinite(node.currentTime) ? node.currentTime : 0;
        player?.destroy();
        player = undefined;
        if (usingAdaptive) fallbackNotice = 'Using standard video playback.';
        usingAdaptive = false;
        node.src = mp4;
        node.load();
        if (restorePosition) {
          node.removeEventListener('loadedmetadata', restorePosition);
          restorePosition = undefined;
        }
        if (position > 0) {
          restorePosition = () => {
            if (!disposed && Number.isFinite(node.duration))
              node.currentTime = Math.min(position, node.duration);
            restorePosition = undefined;
          };
          node.addEventListener('loadedmetadata', restorePosition, { once: true });
        }
      }
      function failed() {
        if (disposed) return;
        if (usingAdaptive) useMp4();
        else error = 'This video could not be loaded. Try again or refresh your access.';
      }
      node.addEventListener('error', failed);
      async function setup() {
        if (!adaptive) {
          useMp4();
          return;
        }
        try {
          const mediaId = privateHlsMediaId(adaptive, location.origin);
          const playlist = privateHlsUrl(adaptive, location.origin, mediaId).href;
          if (node.canPlayType('application/vnd.apple.mpegurl')) {
            usingAdaptive = true;
            node.src = playlist;
            node.load();
            return;
          }
          const { default: HlsPlayer, FetchLoader } = await import('hls.js');
          if (disposed) return;
          if (!HlsPlayer.isSupported()) {
            useMp4();
            return;
          }
          usingAdaptive = true;
          player = new HlsPlayer({
            enableWorker: false,
            autoStartLoad: false,
            capLevelToPlayerSize: true,
            maxBufferLength: 30,
            backBufferLength: 30,
            loader: FetchLoader,
            fetchSetup(context, init) {
              const url = privateHlsUrl(context.url, location.origin, mediaId);
              return new Request(url, {
                ...init,
                credentials: 'same-origin',
                redirect: 'error',
                cache: 'no-store',
              });
            },
          });
          player.on(HlsPlayer.Events.ERROR, (_event, info) => {
            if (info.fatal) failed();
          });
          player.attachMedia(node);
          player.loadSource(playlist);
          // Loading metadata never starts playback; native controls remain the only play action.
          player.startLoad();
        } catch {
          if (!disposed) useMp4();
        }
      }
      void setup();
      return () => {
        disposed = true;
        player?.destroy();
        node.removeEventListener('error', failed);
        if (restorePosition) node.removeEventListener('loadedmetadata', restorePosition);
        node.pause();
        node.removeAttribute('src');
        node.load();
      };
    };
  }
</script>

<div class="private-video">
  <video
    controls
    playsinline
    preload="metadata"
    poster={poster ?? undefined}
    aria-label={label}
    {@attach playback(src, hlsUrl, retry)}
    ><track
      kind="captions"
      src={captionsUrl ?? undefined}
      srclang="en"
      label="English"
      default
    />Your browser does not support this video.</video
  >
  {#if error}<div class="playback-status" role="alert">
      <p>{error}</p>
      <button class="button-secondary" onclick={() => retry++}>Retry video</button>
    </div>
  {:else if fallbackNotice}<span class="sr-only" role="status">{fallbackNotice}</span>{/if}
</div>

<style>
  .private-video {
    position: relative;
    width: 100%;
    height: 100%;
    background: #18251e;
  }
  video {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .playback-status {
    position: absolute;
    inset: 0 0 auto;
    display: grid;
    gap: 8px;
    padding: 16px;
    background: var(--surface);
    color: var(--ink);
    font-size: 12px;
  }
  .playback-status p {
    margin: 0;
  }
  .playback-status button {
    justify-self: start;
  }
</style>
