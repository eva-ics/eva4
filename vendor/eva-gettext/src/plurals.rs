use crate::parser::Error;

use self::Resolver::{Expr, Function};

#[derive(Clone, Debug)]
pub enum Resolver {
    /// A boolean expression
    /// Use Ast::parse to get an Ast
    Expr(Ast),
    /// A function
    Function(fn(u64) -> usize),
}

/// Finds the index of a pattern, outside of parenthesis
fn index_of(src: &str, pat: &'static str) -> Option<usize> {
    src.chars()
        .fold(
            (None, 0, 0, 0),
            |(match_index, i, n_matches, paren_level), ch| {
                if let Some(x) = match_index {
                    (Some(x), i, n_matches, paren_level)
                } else {
                    let new_par_lvl = match ch {
                        '(' => paren_level + 1,
                        ')' => paren_level - 1,
                        _ => paren_level,
                    };

                    if Some(ch) == pat.chars().nth(n_matches) {
                        let length = n_matches + 1;
                        if length == pat.len() && new_par_lvl == 0 {
                            (Some(i - n_matches), i + 1, length, new_par_lvl)
                        } else {
                            (match_index, i + 1, length, new_par_lvl)
                        }
                    } else {
                        (match_index, i + 1, 0, new_par_lvl)
                    }
                }
            },
        )
        .0
}

use self::Ast::{Integer, Not, Op, Ternary, N};
#[derive(Clone, Debug, PartialEq)]
pub enum Ast {
    /// A ternary expression
    /// x ? a : b
    ///
    /// the three Ast<'a> are respectively x, a and b.
    Ternary(Box<Ast>, Box<Ast>, Box<Ast>),
    /// The n variable.
    N,
    /// Integer literals.
    Integer(u64),
    /// Binary operators.
    Op(Operator, Box<Ast>, Box<Ast>),
    /// ! operator.
    Not(Box<Ast>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Operator {
    Equal,
    NotEqual,
    GreaterOrEqual,
    SmallerOrEqual,
    Greater,
    Smaller,
    And,
    Or,
    Modulo,
}

impl Ast {
    #[allow(clippy::cast_possible_truncation)]
    fn resolve(&self, n: u64) -> usize {
        match *self {
            Ternary(ref cond, ref ok, ref nok) => {
                if cond.resolve(n) == 0 {
                    nok.resolve(n)
                } else {
                    ok.resolve(n)
                }
            }
            N => n as usize,
            Integer(x) => x as usize,
            Op(ref op, ref lhs, ref rhs) => match *op {
                Operator::Equal => usize::from(lhs.resolve(n) == rhs.resolve(n)),
                Operator::NotEqual => usize::from(lhs.resolve(n) != rhs.resolve(n)),
                Operator::GreaterOrEqual => usize::from(lhs.resolve(n) >= rhs.resolve(n)),
                Operator::SmallerOrEqual => usize::from(lhs.resolve(n) <= rhs.resolve(n)),
                Operator::Greater => usize::from(lhs.resolve(n) > rhs.resolve(n)),
                Operator::Smaller => usize::from(lhs.resolve(n) < rhs.resolve(n)),
                Operator::And => usize::from(lhs.resolve(n) != 0 && rhs.resolve(n) != 0),
                Operator::Or => usize::from(lhs.resolve(n) != 0 || rhs.resolve(n) != 0),
                Operator::Modulo => lhs.resolve(n) % rhs.resolve(n),
            },
            Not(ref val) => match val.resolve(n) {
                0 => 1,
                _ => 0,
            },
        }
    }

    pub fn parse(src: &str) -> Result<Ast, Error> {
        Self::parse_parens(src.trim())
    }

    fn parse_parens(src: &str) -> Result<Ast, Error> {
        if src.starts_with('(') {
            let end = src[1..src.len() - 1]
                .chars()
                .fold((1, 2), |(level, index), ch| {
                    if level > 0 {
                        if ch == ')' {
                            (level - 1, index + 1)
                        } else if ch == '(' {
                            (level + 1, index + 1)
                        } else {
                            (level, index + 1)
                        }
                    } else {
                        if ch == '(' {
                            (level + 1, index + 1)
                        } else {
                            (level, index)
                        }
                    }
                })
                .1;
            if end == src.len() {
                Ast::parse(src[1..src.len() - 1].trim())
            } else {
                Ast::parse_and(src.trim())
            }
        } else {
            Ast::parse_and(src.trim())
        }
    }

    fn parse_and(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, "&&") {
            Ok(Ast::Op(
                Operator::And,
                Box::new(Ast::parse(&src[0..i])?),
                Box::new(Ast::parse(&src[i + 2..])?),
            ))
        } else {
            Self::parse_or(src)
        }
    }

    fn parse_or(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, "||") {
            Ok(Ast::Op(
                Operator::Or,
                Box::new(Ast::parse(&src[0..i])?),
                Box::new(Ast::parse(&src[i + 2..])?),
            ))
        } else {
            Self::parse_ternary(src)
        }
    }

    fn parse_ternary(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, "?") {
            if let Some(l) = index_of(src, ":") {
                Ok(Ast::Ternary(
                    Box::new(Ast::parse(&src[0..i])?),
                    Box::new(Ast::parse(&src[i + 1..l])?),
                    Box::new(Ast::parse(&src[l + 1..])?),
                ))
            } else {
                Err(Error::PluralParsing)
            }
        } else {
            Self::parse_ge(src)
        }
    }

    fn parse_ge(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, ">=") {
            Ok(Ast::Op(
                Operator::GreaterOrEqual,
                Box::new(Ast::parse(&src[0..i])?),
                Box::new(Ast::parse(&src[i + 2..])?),
            ))
        } else {
            Self::parse_gt(src)
        }
    }

    fn parse_gt(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, ">") {
            Ok(Ast::Op(
                Operator::Greater,
                Box::new(Ast::parse(&src[0..i])?),
                Box::new(Ast::parse(&src[i + 1..])?),
            ))
        } else {
            Self::parse_le(src)
        }
    }

    fn parse_le(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, "<=") {
            Ok(Ast::Op(
                Operator::SmallerOrEqual,
                Box::new(Ast::parse(&src[0..i])?),
                Box::new(Ast::parse(&src[i + 2..])?),
            ))
        } else {
            Self::parse_lt(src)
        }
    }

    fn parse_lt(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, "<") {
            Ok(Ast::Op(
                Operator::Smaller,
                Box::new(Ast::parse(&src[0..i])?),
                Box::new(Ast::parse(&src[i + 1..])?),
            ))
        } else {
            Self::parse_eq(src)
        }
    }

    fn parse_eq(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, "==") {
            Ok(Ast::Op(
                Operator::Equal,
                Box::new(Ast::parse(&src[0..i])?),
                Box::new(Ast::parse(&src[i + 2..])?),
            ))
        } else {
            Self::parse_neq(src)
        }
    }

    fn parse_neq(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, "!=") {
            Ok(Ast::Op(
                Operator::NotEqual,
                Box::new(Ast::parse(&src[0..i])?),
                Box::new(Ast::parse(&src[i + 2..])?),
            ))
        } else {
            Self::parse_mod(src)
        }
    }
    fn parse_mod(src: &str) -> Result<Ast, Error> {
        if let Some(i) = index_of(src, "%") {
            Ok(Ast::Op(
                Operator::Modulo,
                Box::new(Ast::parse(&src[0..i])?),
                Box::new(Ast::parse(&src[i + 1..])?),
            ))
        } else {
            Self::parse_not(src.trim())
        }
    }

    fn parse_not(src: &str) -> Result<Ast, Error> {
        if index_of(src, "!") == Some(0) {
            Ok(Ast::Not(Box::new(Ast::parse(&src[1..])?)))
        } else {
            Self::parse_int(src.trim())
        }
    }

    fn parse_int(src: &str) -> Result<Ast, Error> {
        if let Ok(x) = src.parse::<u64>() {
            Ok(Ast::Integer(x))
        } else {
            Self::parse_n(src.trim())
        }
    }

    fn parse_n(src: &str) -> Result<Ast, Error> {
        if src == "n" {
            Ok(Ast::N)
        } else {
            Err(Error::PluralParsing)
        }
    }
}

impl Resolver {
    /// Returns the number of the correct plural form
    /// for `n` objects, as defined by the rule contained in this resolver.
    pub fn resolve(&self, n: u64) -> usize {
        match *self {
            Expr(ref ast) => ast.resolve(n),
            Function(ref f) => f(n),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expr_resolver() {
        assert_eq!(Expr(N).resolve(42), 42);
    }

    #[test]
    fn test_parser() {
        assert_eq!(
            Ast::parse("n == 42 ? n : 6 && n < 7").expect("Invalid plural"),
            Ast::Op(
                Operator::And,
                Box::new(Ast::Ternary(
                    Box::new(Ast::Op(
                        Operator::Equal,
                        Box::new(Ast::N),
                        Box::new(Ast::Integer(42))
                    )),
                    Box::new(Ast::N),
                    Box::new(Ast::Integer(6))
                )),
                Box::new(Ast::Op(
                    Operator::Smaller,
                    Box::new(Ast::N),
                    Box::new(Ast::Integer(7))
                ))
            )
        );

        assert_eq!(Ast::parse("(n)").expect("Invalid plural"), Ast::N);

        assert_eq!(
            Ast::parse("(n == 1 || n == 2) ? 0 : 1").expect("Invalid plural"),
            Ast::Ternary(
                Box::new(Ast::Op(
                    Operator::Or,
                    Box::new(Ast::Op(
                        Operator::Equal,
                        Box::new(Ast::N),
                        Box::new(Ast::Integer(1))
                    )),
                    Box::new(Ast::Op(
                        Operator::Equal,
                        Box::new(Ast::N),
                        Box::new(Ast::Integer(2))
                    ))
                )),
                Box::new(Ast::Integer(0)),
                Box::new(Ast::Integer(1))
            )
        );

        let ru_plural = "((n%10==1 && n%100!=11) ? 0 : ((n%10 >= 2 && n%10 <=4 && (n%100 < 12 || n%100 > 14)) ? 1 : ((n%10 == 0 || (n%10 >= 5 && n%10 <=9)) || (n%100 >= 11 && n%100 <= 14)) ? 2 : 3))";
        assert!(Ast::parse(ru_plural).is_ok());
    }
}
