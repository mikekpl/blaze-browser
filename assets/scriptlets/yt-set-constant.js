// Blaze scriptlet: yt-set-constant (T044)
// Pins YouTube ad-related config flags to values that disable ad requests
// (uBO "set-constant" style, scoped to the flags that matter).
(() => {
  "use strict";
  if (window.__blazeYtSetConstant) { return; }
  window.__blazeYtSetConstant = true;

  const pin = (obj, path, value) => {
    try {
      const keys = path.split(".");
      const last = keys.pop();
      let target = obj;
      for (const key of keys) {
        if (!target[key] || typeof target[key] !== "object") { target[key] = {}; }
        target = target[key];
      }
      Object.defineProperty(target, last, {
        configurable: false,
        get: () => value,
        set: () => {},
      });
    } catch (_) { /* non-configurable already: leave it */ }
  };

  // Player treats these as "ads already handled".
  pin(window, "ytads.bulleit.enabled", false);
  pin(window, "google_ad_status", 1);
})();
