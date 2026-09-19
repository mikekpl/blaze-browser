// Blaze media enhancer. Injected at document start into every frame (players
// usually live in iframes), independent of Shields.
//
//  1. Subtitles load instantly: caption files for the viewer's languages are
//     pre-fetched into the HTTP cache, so turning captions on never waits on
//     the network. Track modes are never touched — nothing shows unasked.
//  2. Subtitles stay in sync: players that draw captions themselves usually
//     refresh on `timeupdate`, which fires ~4x a second (up to 250 ms late).
//     While such captions are in use, `timeupdate` is driven at frame cadence.
//  3. No stale chrome over the picture: once frames are really being
//     presented, loaders and blocked-ad leftovers are hidden. They are tied to
//     a "frames are flowing" flag, so genuine buffering shows them again.
//
// Hot-path rule: the per-frame callback does one subtraction and returns;
// everything else runs at most ~12x a second, so 4K/60 costs the same as 360p.
(() => {
  "use strict";
  if (window.__blazeMedia) { return; }
  window.__blazeMedia = true;

  const FLOWING = "data-blaze-playing";
  const LOADER = "data-blaze-loader";
  const TICK_MS = 80;          // caption/timeupdate cadence (~12 Hz)
  const STALL_MS = 1200;       // no presented frame for this long → not flowing
  const RESCAN_MS = 2000;

  // ---- 3. overlays --------------------------------------------------------

  // Loaders of the common players, plus leftovers of ad SDKs whose requests
  // were blocked (they otherwise sit on top of the picture, eating clicks).
  const KNOWN_LOADERS = [
    ".vjs-loading-spinner", ".vjs-ad-loading-spinner",
    ".jw-icon-buffer", ".jw-state-buffering .jw-display-icon-container",
    ".shaka-spinner-container", ".fp-waiting", ".fp-wait",
    ".mejs__overlay-loading", ".mejs-overlay-loading",
    ".diplayer-loading-icon", ".art-loading", ".ytp-spinner",
    ".bmpui-ui-buffering-overlay", ".plyr__spinner", ".spinner-three-bounce",
    ".ima-ad-container", "[id$='_ima-ad-container']", ".vjs-ima-ad-container",
  ];
  const PLAYER_ROOTS = ".video-js, .jwplayer, .plyr, .shaka-video-container, "
    + ".flowplayer, .mejs__container, .mejs-container, .dplayer, .art-video-player, "
    + ".html5-video-player, .bmpui-ui-uicontainer, .bitmovinplayer-container, [data-player]";

  const css = [
    KNOWN_LOADERS.map((s) => `[${FLOWING}] ${s}`).join(",")
      + `,[${FLOWING}] [${LOADER}]{display:none !important}`,
    // YouTube: the "more videos" shelf on paused embeds, paid-promotion and
    // suggested-action badges; end-screen cards only while actually playing
    // (they return on pause and at the end, where they are useful).
    ".ytp-pause-overlay,.ytp-paid-content-overlay,.ytp-suggested-action-badge"
      + "{display:none !important}",
    ".html5-video-player.playing-mode .ytp-ce-element{opacity:0 !important;"
      + "pointer-events:none !important}",
  ].join("\n");

  const installStyle = () => {
    const style = document.createElement("style");
    style.textContent = css;
    (document.head || document.documentElement).appendChild(style);
  };

  const rootOf = (video) => {
    const known = video.closest(PLAYER_ROOTS);
    if (known) { return known; }
    // Otherwise: the outermost ancestor still roughly the size of the picture.
    const box = video.getBoundingClientRect();
    const area = Math.max(1, box.width * box.height);
    let root = video.parentElement || video;
    for (let el = root, i = 0; el && el !== document.body && i < 4; el = el.parentElement, i++) {
      const r = el.getBoundingClientRect();
      if (r.width * r.height > area * 1.4) { break; }
      root = el;
    }
    return root;
  };

  // Unknown players: tag things that are named like a loader, sit inside the
  // player and hold nothing the viewer could need.
  const LOADER_NAME = /(^|[-_\s])(spinner|loader|loading|buffering|preloader)([-_\s]|$)/i;
  const tagLoaders = (root, video) => {
    for (const el of root.querySelectorAll("div, span, svg, i, img")) {
      if (el.hasAttribute(LOADER) || el.contains(video)) { continue; }
      const name = `${typeof el.className === "string" ? el.className : ""} ${el.id}`;
      if (!LOADER_NAME.test(name)) { continue; }
      if (el.querySelector("video, audio, iframe, button, input, select, a[href]")) { continue; }
      el.setAttribute(LOADER, "");
    }
  };

  // ---- 1. subtitle prefetch ----------------------------------------------

  const wanted = (navigator.languages && navigator.languages.length
    ? navigator.languages : [navigator.language || "en"])
    .map((l) => String(l).toLowerCase().split("-")[0]);
  const warmed = new Set();

  const warmSubtitles = (video) => {
    const tracks = [...video.querySelectorAll("track[src]")].filter((t) =>
      !t.kind || t.kind === "subtitles" || t.kind === "captions");
    const picks = tracks.filter((t) => t.default
      || wanted.includes(String(t.srclang || "").toLowerCase().split("-")[0]));
    // a lone track is the one that will be used, whatever its language
    if (!picks.length && tracks.length === 1) { picks.push(tracks[0]); }
    for (const track of picks.slice(0, 3)) {
      const url = track.src;
      if (!url || warmed.has(url) || /^(data|blob|javascript):/i.test(url)) { continue; }
      warmed.add(url);
      const cors = video.crossOrigin !== null && video.crossOrigin !== undefined;
      // Same request shape the browser's own track load will use, so the
      // cached response is the one it gets.
      fetch(url, {
        mode: cors ? "cors" : "same-origin",
        credentials: video.crossOrigin === "use-credentials" ? "include" : "same-origin",
        priority: "low",
      }).catch(() => {});
    }
  };

  // ---- 2. caption cadence -------------------------------------------------

  const JS_CAPTION_UI = ".vjs-text-track-display, .jw-captions, .plyr__captions, "
    + ".shaka-text-container, .fp-captions, .mejs__captions-layer, .art-subtitle, "
    + ".dplayer-subtitle, .bmpui-ui-subtitle-overlay";

  // Captions drawn by the page rather than the browser: a track kept "hidden"
  // (cues active, rendering done in JS) or a known JS caption layer.
  const usesJsCaptions = (state) => {
    const tracks = state.video.textTracks;
    if (tracks) {
      for (let i = 0; i < tracks.length; i++) {
        const t = tracks[i];
        if (t.mode === "hidden" && (t.kind === "subtitles" || t.kind === "captions")) {
          return true;
        }
      }
    }
    return !!state.root.querySelector(JS_CAPTION_UI);
  };

  // ---- per-video lifecycle -----------------------------------------------

  const states = new WeakMap();

  const setFlowing = (state, flowing) => {
    if (state.flowing === flowing) { return; }
    state.flowing = flowing;
    if (flowing) { state.root.setAttribute(FLOWING, ""); }
    else { state.root.removeAttribute(FLOWING); }
  };

  const rescan = (state, now) => {
    state.scannedAt = now;
    const root = rootOf(state.video);
    if (root !== state.root) {
      state.root.removeAttribute(FLOWING);
      state.root = root;
      state.flowing = false;
    }
    tagLoaders(state.root, state.video);
    warmSubtitles(state.video);
    state.jsCaptions = usesJsCaptions(state);
  };

  // Runs at most every TICK_MS while frames are being presented.
  const tick = (state, now) => {
    const video = state.video;
    if (now - state.scannedAt > RESCAN_MS) { rescan(state, now); }
    if (video.paused || video.ended || video.seeking) { return; }
    setFlowing(state, true);
    if (state.jsCaptions) { video.dispatchEvent(new Event("timeupdate")); }
  };

  const pump = (state) => {
    const video = state.video;
    if (state.pumping || !video.isConnected) { return; }
    state.pumping = true;
    const onFrame = (now) => {
      state.lastFrame = now;                         // ← the whole per-frame cost
      if (now - state.lastTick >= TICK_MS) {
        state.lastTick = now;
        tick(state, now);
      }
      if (video.paused || video.ended || !video.isConnected) {
        state.pumping = false;
        return;
      }
      schedule();
    };
    const schedule = video.requestVideoFrameCallback
      ? () => video.requestVideoFrameCallback((now) => onFrame(now))
      // no frame callbacks: fall back to the clock actually advancing
      : () => setTimeout(() => {
          const moved = video.currentTime !== state.lastTime;
          state.lastTime = video.currentTime;
          if (moved) { onFrame(performance.now()); } else if (!video.paused) { schedule(); }
          else { state.pumping = false; }
        }, TICK_MS);
    schedule();
  };

  // A frame callback only fires when a frame arrives, so a stall is noticed
  // from outside: no frame for STALL_MS means the loader is wanted again.
  const watchdog = (state) => {
    clearInterval(state.watchdog);
    state.watchdog = setInterval(() => {
      const video = state.video;
      if (!video.isConnected || video.paused || video.ended) {
        clearInterval(state.watchdog);
        state.watchdog = 0;
        setFlowing(state, false);
        return;
      }
      if (performance.now() - state.lastFrame > STALL_MS) { setFlowing(state, false); }
    }, STALL_MS / 2);
  };

  const adopt = (video) => {
    let state = states.get(video);
    if (!state) {
      state = {
        video, root: rootOf(video), flowing: false, pumping: false, jsCaptions: false,
        lastFrame: 0, lastTick: 0, lastTime: -1, scannedAt: 0, watchdog: 0,
      };
      states.set(video, state);
      rescan(state, performance.now());
      // captions switched on/off or tracks added: react now, not at the next rescan
      if (video.textTracks && video.textTracks.addEventListener) {
        const refresh = () => rescan(state, performance.now());
        video.textTracks.addEventListener("change", refresh);
        video.textTracks.addEventListener("addtrack", refresh);
      }
    }
    return state;
  };

  const onEvent = (event) => {
    const video = event.target;
    if (!(video instanceof HTMLVideoElement)) { return; }
    const state = adopt(video);
    switch (event.type) {
      case "playing":
        state.lastFrame = performance.now();
        pump(state);
        watchdog(state);
        break;
      case "loadedmetadata":
        rescan(state, performance.now());
        break;
      case "waiting": case "seeking": case "pause": case "ended": case "emptied":
        setFlowing(state, false);
        break;
    }
  };

  for (const type of ["playing", "loadedmetadata", "waiting", "seeking", "pause", "ended",
                      "emptied"]) {
    document.addEventListener(type, onEvent, true);   // media events don't bubble: capture
  }

  if (document.documentElement) { installStyle(); }
  else { document.addEventListener("readystatechange", installStyle, { once: true }); }
})();
