// Blaze scriptlet: anti-adblock-stealth (T044)
// Presents ad-shaped stubs so adblock detectors ("please disable your ad
// blocker" walls) see what they expect. Applies to all sites.
(() => {
  "use strict";
  if (window.__blazeStealth) { return; }
  window.__blazeStealth = true;

  const define = (name, value) => {
    try {
      if (window[name] !== undefined) { return; }
      Object.defineProperty(window, name, {
        configurable: true,
        writable: false,
        value,
      });
    } catch (_) { /* page defined it first: fine */ }
  };

  // AdSense loader signal checked by most detector scripts.
  define("google_ad_status", 1);

  // adsbygoogle stub: push() must not throw and must mark the slot filled.
  try {
    const adsbygoogle = window.adsbygoogle || [];
    adsbygoogle.loaded = true;
    adsbygoogle.push = function push(arg) {
      try {
        if (arg && typeof arg === "object" && !("length" in arg)) {
          Array.prototype.push.call(this, arg);
        }
      } catch (_) {}
      return 0;
    };
    define("adsbygoogle", adsbygoogle);
  } catch (_) {}

  // Minimal GPT (googletag) surface so detector callbacks still fire.
  try {
    if (!window.googletag || !window.googletag.apiReady) {
      const noop = () => {};
      const chain = new Proxy(noop, {
        get: () => chain,
        apply: () => chain,
      });
      const cmd = window.googletag && window.googletag.cmd ? window.googletag.cmd : [];
      const googletag = {
        apiReady: true,
        cmd: {
          push: (fn) => { try { typeof fn === "function" && fn(); } catch (_) {} return 1; },
        },
        pubads: () => chain,
        defineSlot: () => chain,
        enableServices: noop,
        display: noop,
        destroySlots: noop,
      };
      // Flush callbacks queued before we loaded.
      for (const fn of cmd) { try { typeof fn === "function" && fn(); } catch (_) {} }
      define("googletag", googletag);
    }
  } catch (_) {}
})();
