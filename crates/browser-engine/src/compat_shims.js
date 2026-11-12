// Minimal stand-ins for Web APIs Servo does not implement yet (injected as a user script
// on every page). Sites like Vue/React apps call them unguarded during hydration, and a
// ReferenceError there replaces the server-rendered content with an error screen.
// Each shim is installed only when the native API is missing.
(() => {
  if (typeof window.IntersectionObserver === 'undefined') {
    // Reports every observed element as fully visible once, right after observe(),
    // which is what lazy-loading and "infinite scroll" code needs to render content.
    class IntersectionObserverEntry {
      constructor(target) {
        const rect = target.getBoundingClientRect();
        this.target = target;
        this.time = performance.now();
        this.isIntersecting = true;
        this.intersectionRatio = 1;
        this.boundingClientRect = rect;
        this.intersectionRect = rect;
        this.rootBounds = null;
      }
    }
    class IntersectionObserver {
      constructor(callback, options = {}) {
        if (typeof callback !== 'function') {
          throw new TypeError('IntersectionObserver: callback must be a function');
        }
        this._callback = callback;
        this._targets = new Set();
        this._pending = [];
        this.root = options.root || null;
        this.rootMargin = options.rootMargin || '0px';
        const t = options.threshold;
        this.thresholds = Array.isArray(t) ? t : [typeof t === 'number' ? t : 0];
      }
      observe(target) {
        if (!(target instanceof Element) || this._targets.has(target)) return;
        this._targets.add(target);
        this._pending.push(target);
        if (this._pending.length === 1) {
          setTimeout(() => {
            const entries = this.takeRecords();
            if (entries.length) this._callback(entries, this);
          }, 0);
        }
      }
      unobserve(target) {
        this._targets.delete(target);
        this._pending = this._pending.filter((el) => el !== target);
      }
      disconnect() {
        this._targets.clear();
        this._pending = [];
      }
      takeRecords() {
        const entries = this._pending.map((el) => new IntersectionObserverEntry(el));
        this._pending = [];
        return entries;
      }
    }
    window.IntersectionObserver = IntersectionObserver;
    window.IntersectionObserverEntry = IntersectionObserverEntry;
  }

  if (typeof window.requestIdleCallback === 'undefined') {
    window.requestIdleCallback = (callback, options) => {
      const start = Date.now();
      return setTimeout(() => {
        callback({
          didTimeout: false,
          timeRemaining: () => Math.max(0, 50 - (Date.now() - start)),
        });
      }, Math.min(1, (options && options.timeout) || 1));
    };
    window.cancelIdleCallback = (id) => clearTimeout(id);
  }
})();
