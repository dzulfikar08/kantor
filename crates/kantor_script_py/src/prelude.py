# Kantor's Python bridge (run once per run, before the workbook's modules).
#
# Host objects are `_Obj` wrappers around a native handle. Naming a member reads a property
# right away, or gives a `_Method` to call (see kantor_script::binding). snake_case names
# work: `r.number_format` is `NumberFormat` (underscores are dropped). `_k` is the native side.

_METHOD = object()


def _norm(name):
    return name if name.startswith('_') else name.replace('_', '')


def _member(h, name):
    r = _k.member(h, _norm(name), _METHOD)
    return _Method(h, _norm(name)) if r is _METHOD else r


def _named(kw):
    return [(k, v) for k, v in kw.items()]


class _Method:
    __slots__ = ('_h', '_n')

    def __init__(self, h, n):
        object.__setattr__(self, '_h', h)
        object.__setattr__(self, '_n', n)

    def __call__(self, *args, **kw):
        return _k.call(self._h, self._n, list(args), _named(kw))

    def __str__(self):
        return str(_k.get(self._h, self._n, [], []))

    def __repr__(self):
        return '<method ' + _k.type_name(self._h) + '.' + self._n + '>'


class _Obj:
    __slots__ = ('_h', '__weakref__')

    def __init__(self, h):
        object.__setattr__(self, '_h', h)

    def __del__(self):
        try:
            _k.release(self._h)
        except Exception:
            pass

    def __getattr__(self, name):
        if name.startswith('__'):
            raise AttributeError(name)
        return _member(self._h, name)

    def __setattr__(self, name, value):
        _k.put(self._h, _norm(name), value, [])

    def __call__(self, *args, **kw):
        return _k.get(self._h, '', list(args), _named(kw))

    def __getitem__(self, key):
        return _k.get(self._h, '', list(key) if type(key) is tuple else [key], [])

    def __setitem__(self, key, value):
        _k.put(self._h, '', value, list(key) if type(key) is tuple else [key])

    def __iter__(self):
        return iter(_k.items(self._h))

    def __len__(self):
        return int(_k.get(self._h, 'Count', [], []))

    def __bool__(self):
        return True

    def __eq__(self, other):
        return isinstance(other, _Obj) and _k.same(self._h, other._h)

    def __hash__(self):
        return _k.identity(self._h)

    def __str__(self):
        return str(_k.primitive(self._h))

    def __repr__(self):
        return '<' + _k.type_name(self._h) + '>'

    # A cell's value in arithmetic: `Range("A1") + 1`.
    def __float__(self):
        return float(_k.primitive(self._h))

    def __int__(self):
        return int(_k.primitive(self._h))


class _Live:
    """A global that is looked up each time it is used (`ActiveSheet` is the active sheet
    now). Calling it calls the global with arguments: `Range("A1")`, `Cells(1, 2)`,
    `Worksheets("Data")`."""

    __slots__ = ('_n',)

    def __init__(self, name):
        object.__setattr__(self, '_n', name)

    def _now(self):
        return _k.get(_k.globals, self._n, [], [])

    def __call__(self, *args, **kw):
        return _k.call(_k.globals, self._n, list(args), _named(kw))

    def __getattr__(self, name):
        if name.startswith('__'):
            raise AttributeError(name)
        return getattr(self._now(), name)

    def __setattr__(self, name, value):
        setattr(self._now(), name, value)

    def __getitem__(self, key):
        return self._now()[key]

    def __setitem__(self, key, value):
        self._now()[key] = value

    def __iter__(self):
        return iter(self._now())

    def __len__(self):
        return len(self._now())

    def __bool__(self):
        return True

    def __eq__(self, other):
        return self._now() == other

    def __hash__(self):
        return hash(self._n)

    def __str__(self):
        return str(self._now())

    def __repr__(self):
        return repr(self._now())

    def __float__(self):
        return float(self._now())

    def __int__(self):
        return int(self._now())


def _civil(z):
    # Days since 1970-01-01 to (year, month, day) (H. Hinnant's civil_from_days).
    z += 719468
    era = (z if z >= 0 else z - 146096) // 146097
    doe = z - era * 146097
    yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
    y = yoe + era * 400
    doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
    mp = (5 * doy + 2) // 153
    d = doy - (153 * mp + 2) // 5 + 1
    m = mp + 3 if mp < 10 else mp - 9
    return (y + 1 if m <= 2 else y, m, d)


def _days(y, m, d):
    # (year, month, day) to days since 1970-01-01 (H. Hinnant's days_from_civil).
    y -= m <= 2
    era = (y if y >= 0 else y - 399) // 400
    yoe = y - era * 400
    doy = (153 * (m - 3 if m > 2 else m + 9) + 2) // 5 + d - 1
    doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
    return era * 146097 + doe - 719468


_EPOCH = _days(1899, 12, 30)


class Date(float):
    """A date and time as Excel keeps it: days since 1899-12-30 (the fraction is the time).
    `Date.of(2024, 1, 15, 18, 0)`; `.year`, `.month`, `.day`, `.hour`, `.minute`, `.second`."""

    @classmethod
    def of(cls, year, month, day, hour=0, minute=0, second=0):
        return cls(_days(year, month, day) - _EPOCH + (hour * 3600 + minute * 60 + second) / 86400)

    def _parts(self):
        day = int(self // 1)
        secs = round((self - day) * 86400)
        if secs >= 86400:
            day += 1
            secs -= 86400
        y, m, d = _civil(day + _EPOCH)
        return y, m, d, secs // 3600, secs // 60 % 60, secs % 60

    year = property(lambda self: self._parts()[0])
    month = property(lambda self: self._parts()[1])
    day = property(lambda self: self._parts()[2])
    hour = property(lambda self: self._parts()[3])
    minute = property(lambda self: self._parts()[4])
    second = property(lambda self: self._parts()[5])

    def __repr__(self):
        y, m, d, hh, mm, ss = self._parts()
        if hh or mm or ss:
            return 'Date(%04d-%02d-%02d %02d:%02d:%02d)' % (y, m, d, hh, mm, ss)
        return 'Date(%04d-%02d-%02d)' % (y, m, d)

    __str__ = __repr__


class CellError:
    """An Excel error value: `CVErr(2042)` is `#N/A`."""

    __slots__ = ('code',)

    def __init__(self, code):
        self.code = int(code)

    def __eq__(self, other):
        return isinstance(other, CellError) and other.code == self.code

    def __hash__(self):
        return self.code

    def __repr__(self):
        return _k.error_text(self.code)

    __str__ = __repr__


class KantorError(Exception):
    """A run-time error from the object model, with VBA's error number."""

    def __init__(self, number, description, source=''):
        super().__init__(description)
        self.number = number
        self.description = description
        self.source = source


def _text(v):
    if isinstance(v, (_Obj, _Live)):
        try:
            return repr(v)
        except Exception:
            return '<object>'
    if v is None:
        return 'None'
    return str(v)


def print(*args, sep=' ', end='\n', file=None, flush=False):
    _k.print(sep.join(_text(a) for a in args))


def MsgBox(prompt, buttons=0, title=''):
    return _k.msg_box(_text(prompt), int(buttons), str(title))


def InputBox(prompt, title='', default=''):
    return _k.input_box(_text(prompt), str(title), str(default))


def CVErr(code):
    return CellError(code)


def TypeName(v):
    if isinstance(v, _Live):
        v = v._now()
    if isinstance(v, _Obj):
        return _k.type_name(v._h)
    return _k.type_of(v)


def _describe(e):
    """The exception as text for Rust: number, type, message, frames (innermost last)."""
    number = ''
    message = str(e)
    if isinstance(e, KantorError):
        number = str(e.number)
        message = e.description
    frames = []
    if isinstance(e, SyntaxError):
        frames.append('%s\x1d%d\x1d' % (e.filename or '', e.lineno or 0))
        message = e.msg or message
    tb = e.__traceback__
    while tb is not None:
        code = tb.tb_frame.f_code
        frames.append('%s\x1d%d\x1d%s' % (code.co_filename, tb.tb_lineno, code.co_name))
        tb = tb.tb_next
    return '\x1f'.join([number, type(e).__name__, message, '\x1e'.join(frames)])


# What every module sees: the globals, looked up when used, in VBA's spelling and in
# snake_case (no lower-case `range`: Python's own `range` stays).
_exports = {
    'Date': Date,
    'CellError': CellError,
    'KantorError': KantorError,
    'print': print,
    'MsgBox': MsgBox,
    'msg_box': MsgBox,
    'InputBox': InputBox,
    'input_box': InputBox,
    'CVErr': CVErr,
    'TypeName': TypeName,
}
if _k.globals is not None:
    for _name in _k.global_names:
        _exports[_name] = _Live(_name)
        _snake = ''.join('_' + c.lower() if c.isupper() else c for c in _name).lstrip('_')
        if _snake != 'range':
            _exports.setdefault(_snake, _Live(_name))
