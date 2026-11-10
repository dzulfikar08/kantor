//! Guide formulas (`fmla`, ECMA-376 Part 1 §20.1.9.11). Angles are in
//! 60000ths of a degree.

/// A formula argument: a literal or the name of a guide / built-in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Arg {
    Lit(i64),
    Ref(String),
}

impl Arg {
    pub fn parse(s: &str) -> Self {
        match s.parse() {
            Ok(v) => Self::Lit(v),
            Err(_) => Self::Ref(s.to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// `*/ x y z` = x · y / z
    MulDiv,
    /// `+- x y z` = x + y − z
    AddSub,
    /// `+/ x y z` = (x + y) / z
    AddDiv,
    /// `?: x y z` = y if x > 0, else z
    IfElse,
    Abs,
    /// `at2 x y` = atan2(y, x)
    At2,
    /// `cat2 x y z` = x · cos(atan2(z, y))
    Cat2,
    /// `cos x y` = x · cos(y)
    Cos,
    Max,
    Min,
    /// `mod x y z` = √(x² + y² + z²)
    Mod,
    /// `pin x y z` = y clamped to [x, z]
    Pin,
    /// `sat2 x y z` = x · sin(atan2(z, y))
    Sat2,
    /// `sin x y` = x · sin(y)
    Sin,
    Sqrt,
    /// `tan x y` = x · tan(y)
    Tan,
    Val,
}

impl Op {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "*/" => Self::MulDiv,
            "+-" => Self::AddSub,
            "+/" => Self::AddDiv,
            "?:" => Self::IfElse,
            "abs" => Self::Abs,
            "at2" => Self::At2,
            "cat2" => Self::Cat2,
            "cos" => Self::Cos,
            "max" => Self::Max,
            "min" => Self::Min,
            "mod" => Self::Mod,
            "pin" => Self::Pin,
            "sat2" => Self::Sat2,
            "sin" => Self::Sin,
            "sqrt" => Self::Sqrt,
            "tan" => Self::Tan,
            "val" => Self::Val,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MulDiv => "*/",
            Self::AddSub => "+-",
            Self::AddDiv => "+/",
            Self::IfElse => "?:",
            Self::Abs => "abs",
            Self::At2 => "at2",
            Self::Cat2 => "cat2",
            Self::Cos => "cos",
            Self::Max => "max",
            Self::Min => "min",
            Self::Mod => "mod",
            Self::Pin => "pin",
            Self::Sat2 => "sat2",
            Self::Sin => "sin",
            Self::Sqrt => "sqrt",
            Self::Tan => "tan",
            Self::Val => "val",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Self::Abs | Self::Sqrt | Self::Val => 1,
            Self::At2 | Self::Cos | Self::Max | Self::Min | Self::Sin | Self::Tan => 2,
            _ => 3,
        }
    }
}

/// A parsed `fmla`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Formula {
    pub op: Op,
    pub args: Vec<Arg>,
}

const ANGLE: f64 = 60_000.0;

fn rad(a: f64) -> f64 {
    (a / ANGLE).to_radians()
}

impl Formula {
    /// `None` for an unknown operator; missing arguments count as 0.
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split_whitespace();
        let op = Op::parse(it.next()?)?;
        let mut args: Vec<Arg> = it.take(op.arity()).map(Arg::parse).collect();
        args.resize(op.arity(), Arg::Lit(0));
        Some(Self { op, args })
    }

    /// The value, with arguments resolved by `get`.
    pub fn eval(&self, get: impl Fn(&Arg) -> f64) -> f64 {
        let a = |i: usize| self.args.get(i).map_or(0.0, &get);
        let (x, y, z) = (a(0), a(1), a(2));
        let div = |n: f64, d: f64| if d == 0.0 { 0.0 } else { n / d };
        match self.op {
            Op::MulDiv => div(x * y, z),
            Op::AddSub => x + y - z,
            Op::AddDiv => div(x + y, z),
            Op::IfElse => {
                if x > 0.0 {
                    y
                } else {
                    z
                }
            }
            Op::Abs => x.abs(),
            Op::At2 => y.atan2(x).to_degrees() * ANGLE,
            Op::Cat2 => x * z.atan2(y).cos(),
            Op::Cos => x * rad(y).cos(),
            Op::Max => x.max(y),
            Op::Min => x.min(y),
            Op::Mod => (x * x + y * y + z * z).sqrt(),
            Op::Pin => {
                if y < x {
                    x
                } else if y > z {
                    z
                } else {
                    y
                }
            }
            Op::Sat2 => x * z.atan2(y).sin(),
            Op::Sin => x * rad(y).sin(),
            Op::Sqrt => x.max(0.0).sqrt(),
            Op::Tan => x * rad(y).tan(),
            Op::Val => x,
        }
    }
}

impl std::fmt::Display for Arg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lit(v) => write!(f, "{v}"),
            Self::Ref(n) => f.write_str(n),
        }
    }
}

/// The `fmla` text.
impl std::fmt::Display for Formula {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.op.as_str())?;
        for a in &self.args {
            write!(f, " {a}")?;
        }
        Ok(())
    }
}
