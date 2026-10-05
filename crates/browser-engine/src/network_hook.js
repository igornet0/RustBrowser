// Automation network recorder (injected as a user script in --automation-content mode).
// Wraps fetch() and XMLHttpRequest to keep bounded copies of text/JSON responses in
// window.__rbNet, and tracks in-flight requests for "network idle" waits.
// It only observes: requests and responses reach the page unchanged.
(() => {
  if (window.__rbNet) return;
  const MAX_ENTRIES = 100;
  const MAX_BODY = 262144;
  const MAX_TOTAL = 2097152;
  const net = { inflight: 0, last: Date.now(), total: 0, entries: [] };
  Object.defineProperty(window, '__rbNet', { value: net, enumerable: false });

  const textual = (ct) => /json|graphql|javascript|text\/plain|xml/i.test(ct || '');
  const record = (entry) => {
    if (net.entries.length < MAX_ENTRIES) net.entries.push(entry);
  };
  const keep = (text) => {
    if (typeof text !== 'string') return null;
    const room = Math.max(0, MAX_TOTAL - net.total);
    const kept = text.slice(0, Math.min(MAX_BODY, room));
    net.total += kept.length;
    return kept;
  };
  const started = () => {
    net.inflight += 1;
    net.last = Date.now();
    return net.last;
  };
  const finished = () => {
    net.inflight = Math.max(0, net.inflight - 1);
    net.last = Date.now();
  };
  const bodyPreview = (body) => (typeof body === 'string' ? body.slice(0, 2048) : null);

  const origFetch = window.fetch;
  if (typeof origFetch === 'function') {
    window.fetch = function (input, init) {
      const url = String((input && input.url) || input);
      const method = String((init && init.method) || (input && input.method) || 'GET').toUpperCase();
      const requestBody = bodyPreview(init && init.body);
      const t0 = started();
      return origFetch.apply(this, arguments).then(
        (res) => {
          const ct = (res.headers && res.headers.get('content-type')) || '';
          const entry = {
            kind: 'fetch', url: res.url || url, method, status: res.status, content_type: ct,
            request_body: requestBody, duration_ms: Date.now() - t0, body: null, body_length: null,
          };
          record(entry);
          if (textual(ct)) {
            res.clone().text().then(
              (text) => { entry.body = keep(text); entry.body_length = text.length; finished(); },
              finished,
            );
          } else {
            finished();
          }
          return res;
        },
        (err) => {
          record({ kind: 'fetch', url, method, error: String(err), duration_ms: Date.now() - t0 });
          finished();
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
      this.__rbInfo = { method: String(method || 'GET').toUpperCase(), url: String(url) };
      return open.apply(this, arguments);
    };
    XHR.prototype.send = function (body) {
      const info = this.__rbInfo || {};
      const t0 = started();
      this.addEventListener('loadend', () => {
        const ct = this.getResponseHeader('content-type') || '';
        let text = null;
        try {
          if (textual(ct) && (this.responseType === '' || this.responseType === 'text')) {
            text = this.responseText;
          }
        } catch (e) { /* opaque response */ }
        record({
          kind: 'xhr', url: this.responseURL || info.url, method: info.method || 'GET',
          status: this.status, content_type: ct, request_body: bodyPreview(body),
          duration_ms: Date.now() - t0, body: keep(text), body_length: text ? text.length : null,
        });
        finished();
      });
      return send.apply(this, arguments);
    };
  }
})();
