// Blaze scriptlet: yt-ad-prune (T044)
// Strips ad payloads (pre/mid-roll, ad slots) from YouTube player responses
// before the player reads them. Applies to youtube.com-family hosts only.
(() => {
  "use strict";
  if (window.__blazeYtAdPrune) { return; }
  window.__blazeYtAdPrune = true;

  const AD_KEYS = [
    "adPlacements",
    "adSlots",
    "playerAds",
    "adBreakHeartbeatParams",
    "importantComments",
  ];

  const prune = (obj) => {
    try {
      if (!obj || typeof obj !== "object") { return obj; }
      for (const key of AD_KEYS) {
        if (key in obj) { delete obj[key]; }
      }
      if (obj.playerResponse) { prune(obj.playerResponse); }
      if (obj.playerConfig && obj.playerConfig.daiConfig) {
        delete obj.playerConfig.daiConfig;
      }
    } catch (_) { /* never break the page */ }
    return obj;
  };

  // Initial payload is assigned to a global before the player boots.
  let initialResponse;
  try {
    Object.defineProperty(window, "ytInitialPlayerResponse", {
      configurable: true,
      get: () => initialResponse,
      set: (value) => { initialResponse = prune(value); },
    });
  } catch (_) { /* already defined non-configurable: skip */ }

  // Subsequent player data arrives via fetch().json() and JSON.parse.
  const origParse = JSON.parse;
  JSON.parse = function parse(...args) {
    return prune(origParse.apply(this, args));
  };

  const origJson = Response.prototype.json;
  Response.prototype.json = function json(...args) {
    return origJson.apply(this, args).then(prune);
  };
})();
