// Diagnostics-only user script (RUST_BROWSER_DIAG=1). Records console output,
// uncaught errors, fetch/XHR, visibility and hCaptcha lifecycle into
// window.__rb_log; the embedder drains it once per second over tracing.
(() => {
  const MAX = 400;
  const push = (kind, data) => {
    try {
      const list = window.__rb_log || (window.__rb_log = []);
      list.push({ k: kind, t: Date.now(), d: String(data).slice(0, 600) });
      if (list.length > MAX) list.splice(0, list.length - MAX);
    } catch (_) {}
  };
  const fmt = (v) => {
    try {
      if (typeof v === 'string') return v;
      if (v instanceof Error) return `${v.name}: ${v.message}`;
      return JSON.stringify(v);
    } catch (_) {
      return String(v);
    }
  };

  // Heartbeat: if this stops advancing while the UI is alive, the page's JS
  // thread is blocked (not just waiting on a promise).
  window.__rb_beat = Date.now();
  setInterval(() => {
    window.__rb_beat = Date.now();
  }, 500);

  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    const original = console[level];
    if (typeof original !== 'function') continue;
    console[level] = function (...args) {
      push('console.' + level, args.map(fmt).join(' '));
      return original.apply(this, args);
    };
  }

  addEventListener('error', (e) => {
    push(
      'window.error',
      `${e.message || ''} @ ${e.filename || ''}:${e.lineno || 0}:${e.colno || 0}`,
    );
  });
  addEventListener('unhandledrejection', (e) => {
    push('unhandledrejection', fmt(e && e.reason));
  });

  const originalFetch = window.fetch;
  if (typeof originalFetch === 'function') {
    window.fetch = function (input, init) {
      let url;
      try {
        url = typeof input === 'string' ? input : (input && input.url) || String(input);
      } catch (_) {
        url = '?';
      }
      const method = (init && init.method) || (input && input.method) || 'GET';
      const started = Date.now();
      push('fetch.start', `${method} ${url}`);
      let promise;
      try {
        promise = originalFetch.apply(this, arguments);
      } catch (err) {
        push('fetch.throw', `${url} ${fmt(err)}`);
        throw err;
      }
      return promise.then(
        (res) => {
          push('fetch.done', `${url} ${res.status} ${Date.now() - started}ms`);
          return res;
        },
        (err) => {
          push('fetch.fail', `${url} ${(err && err.name) || 'error'} ${Date.now() - started}ms`);
          throw err;
        },
      );
    };
  }

  const XHR = window.XMLHttpRequest;
  if (XHR && XHR.prototype) {
    const open = XHR.prototype.open;
    const send = XHR.prototype.send;
    XHR.prototype.open = function (method, url) {
      try {
        this.__rb = { method, url: String(url), t0: Date.now() };
      } catch (_) {}
      return open.apply(this, arguments);
    };
    XHR.prototype.send = function () {
      const meta = this.__rb;
      if (meta) {
        push('xhr.start', `${meta.method} ${meta.url}`);
        const done = (kind) => () => {
          try {
            push(kind, `${meta.url} ${this.status} ${Date.now() - meta.t0}ms`);
          } catch (_) {}
        };
        this.addEventListener('load', done('xhr.done'));
        this.addEventListener('error', done('xhr.error'));
        this.addEventListener('timeout', done('xhr.timeout'));
        this.addEventListener('abort', done('xhr.abort'));
      }
      return send.apply(this, arguments);
    };
  }

  // hCaptcha: record when verification starts/finishes. A missing
  // "hcaptcha.resolve/reject" is exactly the "Send code hangs" signature.
  let hcaptchaValue;
  try {
    Object.defineProperty(window, 'hcaptcha', {
      configurable: true,
      get() {
        return hcaptchaValue;
      },
      set(value) {
        if (!value || value.__rbWrapped) {
          hcaptchaValue = value;
          return;
        }
        try {
          value.__rbWrapped = true;
          const originalExecute = value.execute;
          if (typeof originalExecute === 'function') {
            value.execute = function (id, options) {
              const started = Date.now();
              const async = !!(options && options.async);
              push('hcaptcha.execute', `id=${id} async=${async}`);
              let result;
              try {
                result = originalExecute.call(this, id, options);
              } catch (err) {
                push('hcaptcha.throw', fmt(err));
                throw err;
              }
              if (result && typeof result.then === 'function') {
                return result.then(
                  (val) => {
                    push(
                      'hcaptcha.resolve',
                      `${Date.now() - started}ms ${val && val.response ? 'token' : fmt(val)}`,
                    );
                    return val;
                  },
                  (err) => {
                    push('hcaptcha.reject', `${Date.now() - started}ms ${fmt(err)}`);
                    throw err;
                  },
                );
              }
              push('hcaptcha.return', fmt(result));
              return result;
            };
          }
          const originalRender = value.render;
          if (typeof originalRender === 'function') {
            value.render = function (el, options) {
              push(
                'hcaptcha.render',
                `size=${(options && options.size) || '?'} el=${(el && el.id) || el}`,
              );
              return originalRender.apply(this, arguments);
            };
          }
        } catch (_) {}
        hcaptchaValue = value;
      },
    });
  } catch (_) {}

  document.addEventListener('visibilitychange', () => {
    push('visibilitychange', document.visibilityState);
  });
  addEventListener('focus', () => push('window.focus', ''));
  addEventListener('blur', () => push('window.blur', ''));
  addEventListener('pagehide', () => push('pagehide', ''));
  addEventListener('pageshow', () => push('pageshow', ''));
  push('diag.installed', location.href);
})();
