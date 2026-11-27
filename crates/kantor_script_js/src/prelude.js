// Kantor's JavaScript bridge (evaluated once per run, before the workbook's modules).
//
// Host objects are proxies over a function (so they can be called: `Worksheets("Data")`,
// `ws.Cells(2, 1)` take the default member) holding a native handle. Naming a member reads a
// property right away, or gives a function for a method (see kantor_script::binding). `N` is
// the native side; `__kantor` keeps what Rust needs from here.
(function (N) {
  'use strict';
  const HANDLES = new WeakMap();
  const METHOD = {};
  // Names JavaScript itself probes for on any object: never members.
  const PROBES = new Set([
    'then', 'toJSON', 'constructor', 'prototype', '__proto__', 'inspect', 'asymmetricMatch',
    '$$typeof', 'nodeType', 'call', 'apply', 'bind',
  ]);

  // Positional arguments, and named ones from a trailing plain object:
  // `r.copy({ destination: ws.range("A1") })`.
  function split(args) {
    if (args.length) {
      const last = args[args.length - 1];
      if (last !== null && typeof last === 'object' && !Array.isArray(last) &&
          Object.getPrototypeOf(last) === Object.prototype) {
        return [args.slice(0, -1), last];
      }
    }
    return [args, undefined];
  }

  function method(h, name) {
    const f = function (...args) {
      const [p, n] = split(args);
      return N.call(h, name, p, n);
    };
    // `"At " + r.address` reads it without arguments.
    f[Symbol.toPrimitive] = function () { return N.get(h, name, [], undefined); };
    return f;
  }

  const handler = {
    get(t, key) {
      const h = t.h;
      if (typeof key === 'symbol') {
        if (key === Symbol.iterator) return function () { return N.items(h)[Symbol.iterator](); };
        if (key === Symbol.toPrimitive) return function () { return N.primitive(h); };
        if (key === Symbol.toStringTag) return N.typeName(h);
        return undefined;
      }
      if (key === 'toString') return function () { return String(N.primitive(h)); };
      if (key === 'valueOf') return function () { return N.primitive(h); };
      if (PROBES.has(key)) return undefined;
      const r = N.member(h, key, METHOD);
      return r === METHOD ? method(h, key) : r;
    },
    set(t, key, value) {
      if (typeof key === 'symbol') return false;
      N.put(t.h, key, value);
      return true;
    },
    apply(t, self, args) {
      const [p, n] = split(args);
      return N.get(t.h, '', p, n);
    },
    deleteProperty() { return false; },
    defineProperty() { return false; },
  };

  function wrap(h) {
    const t = () => {};
    t.h = h;
    const p = new Proxy(t, handler);
    HANDLES.set(p, h);
    return p;
  }

  // An Excel error value (`#N/A`, `#DIV/0!`): `CVErr(2042)`.
  class CellError {
    constructor(code) { this.code = code | 0; }
    toString() { return N.errorText(this.code); }
  }

  // A run-time error from the object model, with VBA's number.
  class KantorError extends Error {
    constructor(number, description, source) {
      super(description);
      this.number = number;
      this.description = description;
      this.source = source;
    }
  }
  Object.defineProperty(KantorError.prototype, 'name', { value: 'KantorError' });

  function text(v, depth) {
    if (HANDLES.has(v)) return '[' + N.typeName(HANDLES.get(v)) + ']';
    if (v === null || v === undefined) return String(v);
    if (Array.isArray(v)) {
      if (depth > 3) return '[…]';
      return '[' + v.map((x) => text(x, depth + 1)).join(', ') + ']';
    }
    if (v instanceof Date) return isNaN(v) ? 'Invalid Date' : v.toLocaleString();
    if (v instanceof Error) return String(v);
    if (typeof v === 'object' && !(v instanceof CellError)) {
      try { return JSON.stringify(v); } catch (e) { return String(v); }
    }
    return String(v);
  }
  const log = function (...args) { N.print(args.map((a) => text(a, 0)).join(' ')); };

  const kit = {
    wrap,
    classify(v) {
      const h = HANDLES.get(v);
      if (h) return h;
      if (v instanceof Date) {
        if (isNaN(v)) return ['n'];
        return ['d', v.getFullYear(), v.getMonth() + 1, v.getDate(), v.getHours(),
          v.getMinutes(), v.getSeconds(), v.getMilliseconds()];
      }
      if (v instanceof CellError) return ['e', v.code];
      return undefined;
    },
    date(y, m, d, hh, mm, ss) {
      const r = new Date(2000, 0, 1, hh, mm, ss);
      r.setFullYear(y, m - 1, d);
      return r;
    },
    cellError(code) { return new CellError(code); },
    error(number, description, source) { return new KantorError(number, description, source); },
  };
  Object.defineProperty(globalThis, '__kantor', { value: kit });

  function define(name, get) {
    Object.defineProperty(globalThis, name, {
      configurable: true,
      enumerable: false,
      get,
      // A script may reuse the name for its own variable.
      set(v) {
        Object.defineProperty(globalThis, name, {
          value: v, writable: true, configurable: true, enumerable: true,
        });
      },
    });
  }
  function both(name, value) {
    globalThis[name] = value;
    globalThis[name[0].toLowerCase() + name.slice(1)] = value;
  }

  both('MsgBox', function (prompt, buttons, title) {
    if (buttons !== null && typeof buttons === 'object') {
      title = buttons.title;
      buttons = buttons.buttons;
    }
    return N.msgBox(text(prompt, 0), buttons | 0, title === undefined ? '' : String(title));
  });
  both('InputBox', function (prompt, title, def) {
    if (title !== null && typeof title === 'object') {
      def = title.default;
      title = title.title;
    }
    return N.inputBox(text(prompt, 0), title === undefined ? '' : String(title),
      def === undefined ? '' : String(def));
  });
  both('CVErr', function (code) { return new CellError(code); });
  both('TypeName', function (v) {
    if (HANDLES.has(v)) return N.typeName(HANDLES.get(v));
    return N.typeOf(v);
  });
  globalThis.CellError = CellError;
  globalThis.KantorError = KantorError;
  globalThis.print = log;
  globalThis.console = { log, info: log, warn: log, error: log, debug: log };

  if (N.globals !== undefined) {
    const G = wrap(N.globals);
    for (const name of N.globalNames) {
      const get = function () { return G[name]; };
      define(name, get);
      const lower = name[0].toLowerCase() + name.slice(1);
      if (lower !== name) define(lower, get);
    }
    // Other names (the object model's constants `xlUp`, `xlValues`; the Application's
    // other members) are looked up when a script names one it doesn't define.
    try {
      const base = Object.getPrototypeOf(globalThis);
      Object.setPrototypeOf(globalThis, new Proxy(base, {
        get(t, key, recv) {
          if (typeof key === 'symbol' || key in t) return Reflect.get(t, key, recv);
          const r = N.lookup(key, METHOD);
          if (r === METHOD) return method(N.globals, key);
          if (r === undefined) throw new ReferenceError(key + ' is not defined');
          return r;
        },
        has(t, key) {
          if (typeof key === 'symbol' || key in t) return key in t;
          return N.known(key);
        },
      }));
    } catch (e) {
      // The named globals above still work.
    }
  }
})
