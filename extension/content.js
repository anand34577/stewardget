// Alt+click on a link keeps that download in the browser (like IDM). Chrome
// downloads a link on Alt+click by itself; this tells StewardGet to stay out of it.
// Nothing about the page is read or sent anywhere.

(() => {
  const ext = globalThis.browser ?? globalThis.chrome;
  document.addEventListener(
    "click",
    (e) => {
      if (!e.altKey) return;
      const link = e.target instanceof Element ? e.target.closest("a[href], area[href]") : null;
      if (!link?.href) return;
      try {
        ext.runtime.sendMessage({ type: "bypass", url: link.href }).catch(() => {});
      } catch {
        /* the extension was reloaded; nothing to do */
      }
    },
    true,
  );
})();
