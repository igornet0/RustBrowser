// Minimal stand-ins for Web APIs Servo does not implement yet (injected as a user script
// on every page). Sites like Vue/React apps call them unguarded during hydration, and a
// ReferenceError there replaces the server-rendered content with an error screen.
// Each shim is installed only when the native API is missing.
(() => {
  // Servo 0.5 does not accept the SVG-ish `pointer-events: all` value used by
  // many design systems (e.g. DeepSeek's `.ds-modal { pointer-events: all }`).
  // The declaration is dropped, the modal inherits `pointer-events: none` from
  // its wrapper, and every control inside a dialog becomes click-through —
  // visually fine, completely unclickable. Dialogs must stay interactive.
  try {
    const style = document.createElement('style');
    style.setAttribute('data-rb-shim', 'dialog-pointer-events');
    style.textContent =
      // Dialogs, popovers, menus and the design-system wrappers that rely on
      // `pointer-events: all` (which Servo drops).
      '[role="dialog"], [aria-modal="true"], [role="alertdialog"], ' +
      '[role="menu"], [role="listbox"], [role="tooltip"], ' +
      '.ds-modal-wrapper .ds-modal, .ds-floating-position-wrapper, ' +
      '.ds-notification-container .ds-notification, .ds-toast, ' +
      '.ds-scroll-area__gutters .ds-scroll-area__horizontal-bar, ' +
      '.ds-scroll-area__gutters .ds-scroll-area__vertical-bar, ' +
      '[class*="popover"], [class*="dropdown"], [class*="floating-position"], ' +
      '[class*="context-menu"] { pointer-events: auto !important; }';
    (document.head || document.documentElement).appendChild(style);
  } catch (_) {}

  // Servo 0.5 leaves Document.visibilityState stuck at "hidden" after an
  // initial load (only history reactivation sets it to "visible"). Captcha
  // widgets, video players, chat apps and analytics pause or refuse to start
  // when the document looks hidden — e.g. hCaptcha "Send code" never resolves.
  try {
    if (document.visibilityState !== 'visible') {
      Object.defineProperty(Document.prototype, 'visibilityState', {
        configurable: true,
        get: function () {
          return 'visible';
        },
      });
      Object.defineProperty(Document.prototype, 'hidden', {
        configurable: true,
        get: function () {
          return false;
        },
      });
      const announce = () => {
        try {
          document.dispatchEvent(new Event('visibilitychange', { bubbles: true }));
        } catch (_) {}
      };
      if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', announce, { once: true });
      } else {
        setTimeout(announce, 0);
      }
    }
  } catch (_) {}

  // Servo reports 0×0 for the outer window; sites use it for modal placement
  // and anti-bot heuristics.
  try {
    if (window.outerWidth === 0) {
      Object.defineProperty(window, 'outerWidth', {
        configurable: true,
        get: function () {
          return window.innerWidth || document.documentElement.clientWidth || 0;
        },
      });
    }
    if (window.outerHeight === 0) {
      Object.defineProperty(window, 'outerHeight', {
        configurable: true,
        get: function () {
          return window.innerHeight || document.documentElement.clientHeight || 0;
        },
      });
    }
  } catch (_) {}

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

  // Servo reports a 0×0 screen unless the embedder supplies geometry. Sites use
  // these for layout breakpoints and modal sizing, so fall back to the viewport.
  if (typeof screen !== 'undefined' && screen && (screen.width === 0 || screen.height === 0)) {
    const width = () => window.innerWidth || document.documentElement.clientWidth || 0;
    const height = () => window.innerHeight || document.documentElement.clientHeight || 0;
    let installed = false;
    // Prefer a Proxy: it keeps every native Screen property (orientation,
    // colorDepth, …) working while only overriding the bogus 0 sizes.
    if (typeof Proxy === 'function') {
      try {
        const real = screen;
        const fakeOrientation = {
          type: 'landscape-primary',
          angle: 90,
          onchange: null,
          lock: async () => {},
          unlock: () => {},
          addEventListener() {},
          removeEventListener() {},
        };
        const proxy = new Proxy(real, {
          get(target, prop) {
            if (prop === 'width' || prop === 'availWidth') return width();
            if (prop === 'height' || prop === 'availHeight') return height();
            if (prop === 'availLeft' || prop === 'availTop') return 0;
            if (prop === 'orientation') {
              return Reflect.get(target, prop, target) || fakeOrientation;
            }
            const value = Reflect.get(target, prop, target);
            return typeof value === 'function' ? value.bind(target) : value;
          },
          has(target, prop) {
            return (
              prop === 'width' ||
              prop === 'height' ||
              prop === 'availWidth' ||
              prop === 'availHeight' ||
              prop === 'availLeft' ||
              prop === 'availTop' ||
              prop in target
            );
          },
          getOwnPropertyDescriptor(target, prop) {
            if (prop === 'width' || prop === 'availWidth') {
              return { configurable: true, enumerable: true, value: width() };
            }
            if (prop === 'height' || prop === 'availHeight') {
              return { configurable: true, enumerable: true, value: height() };
            }
            return Reflect.getOwnPropertyDescriptor(target, prop);
          },
        });
        Object.defineProperty(window, 'screen', {
          get: () => proxy,
          configurable: true,
        });
        installed = true;
      } catch (_) {}
    }
    if (!installed) {
      const define = (target, prop, get) => {
        try {
          Object.defineProperty(target, prop, { get, configurable: true });
          return true;
        } catch (_) {
          return false;
        }
      };
      define(screen, 'width', width);
      define(screen, 'height', height);
      define(screen, 'availWidth', width);
      define(screen, 'availHeight', height);
    }
  }

  // Servo has no VisualViewport. Captcha widgets and mobile-aware layouts probe it.
  if (typeof window.visualViewport === 'undefined') {
    const listeners = new Map();
    const emit = (type) => {
      const set = listeners.get(type);
      if (!set) return;
      for (const fn of [...set]) {
        try {
          fn.call(vv, { type, target: vv });
        } catch (_) {}
      }
    };
    const vv = {
      get width() { return window.innerWidth || 0; },
      get height() { return window.innerHeight || 0; },
      get offsetLeft() { return 0; },
      get offsetTop() { return 0; },
      get pageLeft() { return window.scrollX || 0; },
      get pageTop() { return window.scrollY || 0; },
      get scale() { return 1; },
      onresize: null,
      onscroll: null,
      addEventListener(type, fn) {
        if (typeof fn !== 'function') return;
        const set = listeners.get(type) || new Set();
        set.add(fn);
        listeners.set(type, set);
        if (!vv._wired) {
          vv._wired = true;
          window.addEventListener('resize', () => emit('resize'));
          window.addEventListener('scroll', () => emit('scroll'), true);
        }
      },
      removeEventListener(type, fn) {
        const set = listeners.get(type);
        if (set) set.delete(fn);
      },
      dispatchEvent(event) {
        emit(event && event.type);
        return true;
      },
    };
    try {
      Object.defineProperty(window, 'visualViewport', {
        value: vv,
        configurable: true,
      });
    } catch (_) {}
  }

  // Minimal high-entropy client hints. Captcha/anti-bot SDKs branch on this.
  if (!navigator.userAgentData) {
    const ua = navigator.userAgent || '';
    const platform = /Windows/i.test(ua)
      ? 'Windows'
      : /Android/i.test(ua)
        ? 'Android'
        : /iPhone|iPad|iPod/i.test(ua)
          ? 'iOS'
          : /Mac/i.test(ua)
            ? 'macOS'
            : 'Linux';
    const brands = [
      { brand: 'Chromium', version: '140' },
      { brand: 'Not?A_Brand', version: '99' },
    ];
    try {
      Object.defineProperty(navigator, 'userAgentData', {
        configurable: true,
        value: {
          brands,
          mobile: /Android|iPhone|iPad|iPod/i.test(ua),
          platform,
          getHighEntropyValues: async () => ({
            brands,
            mobile: /Android|iPhone|iPad|iPod/i.test(ua),
            platform,
            platformVersion: '10.0.0',
            architecture: 'arm',
            bitness: '64',
            model: '',
            uaFullVersion: '140.0.0.0',
            fullVersionList: brands,
          }),
          toJSON() {
            return { brands, mobile: this.mobile, platform };
          },
        },
      });
    } catch (_) {}
  }

  if (!navigator.connection) {
    try {
      Object.defineProperty(navigator, 'connection', {
        configurable: true,
        value: {
          effectiveType: '4g',
          rtt: 50,
          downlink: 10,
          saveData: false,
          addEventListener() {},
          removeEventListener() {},
        },
      });
    } catch (_) {}
  }
  if (typeof navigator.deviceMemory === 'undefined') {
    try {
      Object.defineProperty(navigator, 'deviceMemory', {
        configurable: true,
        value: 8,
      });
    } catch (_) {}
  }

  // Some animation/visibility code calls getAnimations unguarded.
  if (typeof Element !== 'undefined' && !Element.prototype.getAnimations) {
    Element.prototype.getAnimations = function () {
      return [];
    };
  }

  // Scheduler API: enough for libraries that feature-detect it.
  if (typeof window.scheduler === 'undefined') {
    window.scheduler = {
      postTask(callback, options = {}) {
        return new Promise((resolve, reject) => {
          setTimeout(() => {
            try {
              resolve(callback());
            } catch (err) {
              reject(err);
            }
          }, Math.max(0, options.delay || 0));
        });
      },
      yield() {
        return new Promise((resolve) => setTimeout(resolve, 0));
      },
    };
  }
})();
