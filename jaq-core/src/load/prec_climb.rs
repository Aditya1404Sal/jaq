//! Precedence climbing for parsing expressions with binary operators.
//!
//! This allows you to parse expressions that are
//! separated by binary operators with precedence and associativity.
//! For example, in the expression `1 + 2 * 3`, we usually want to
//! parse this into `1 + (2 * 3)`, not `(1 + 2) * 3`.
//! This is handled by saying that `*` has higher *precedence* than `+`.
//! Also, when we have a power operator `^`, we want
//! `2 ^ 3 ^ 4` to mean `(2 ^ 3) ^ 4`, not `2 ^ (3 ^ 4)`.
//! This is handled by saying that `^` is *left-associative*.
//!
//! This was adapted from
//! <https://ycpcs.github.io/cs340-fall2017/lectures/lecture06.html#implementation>.

use alloc::vec::Vec;

/// Associativity of an operator.
pub enum Associativity {
    /// `(x + y) + z`
    Left,
    /// `x + (y + z)`
    Right,
}

/// Binary operator.
pub trait Op {
    /// "Stickiness" of the operator
    fn precedence(&self) -> usize;
    /// Is the operator left- or right-associative?
    fn associativity(&self) -> Associativity;
}

/// An expression that can be built from other expressions with some operator.
pub trait Expr<O: Op> {
    /// Combine two expressions with an operator.
    fn from_op(lhs: Self, op: O, rhs: Self) -> Self;
}

/// Perform precedence climbing.
///
/// This is the shunting-yard form of precedence climbing: operators wait on a stack until an
/// operator that binds less tightly arrives, so a long chain of operators (`1 + 1 + ...`,
/// `f | g | ...`) is parsed without recursing once per operator.
pub fn climb<O: Op, T: Expr<O>>(head: T, iter: impl IntoIterator<Item = (O, T)>) -> T {
    let mut operands = Vec::from([head]);
    let mut ops: Vec<O> = Vec::new();
    for (op, rhs) in iter {
        while let Some(top) = ops.last() {
            let (top_prec, prec) = (top.precedence(), op.precedence());
            let left = matches!(op.associativity(), Associativity::Left);
            if top_prec > prec || (top_prec == prec && left) {
                reduce(&mut operands, &mut ops);
            } else {
                break;
            }
        }
        ops.push(op);
        operands.push(rhs);
    }
    while !ops.is_empty() {
        reduce(&mut operands, &mut ops);
    }
    operands.pop().expect("one operand is left")
}

/// Replace the two topmost operands by the topmost operator applied to them.
fn reduce<O: Op, T: Expr<O>>(operands: &mut Vec<T>, ops: &mut Vec<O>) {
    let (Some(op), Some(rhs), Some(lhs)) = (ops.pop(), operands.pop(), operands.pop()) else {
        unreachable!("an operator always has two operands")
    };
    operands.push(T::from_op(lhs, op, rhs));
}

/// Simple arithmetic expressions
#[test]
fn test() {
    enum Arith {
        Add,
        Sub,
        Mul,
        Div,
    }

    impl Op for Arith {
        fn precedence(&self) -> usize {
            match self {
                Arith::Add | Arith::Sub => 0,
                Arith::Mul | Arith::Div => 1,
            }
        }

        fn associativity(&self) -> Associativity {
            Associativity::Right
        }
    }

    impl Expr<Arith> for isize {
        fn from_op(lhs: Self, op: Arith, rhs: Self) -> Self {
            match op {
                Arith::Add => lhs + rhs,
                Arith::Sub => lhs - rhs,
                Arith::Mul => lhs * rhs,
                Arith::Div => lhs / rhs,
            }
        }
    }

    use Arith::{Add, Div, Mul, Sub};
    // 1 + 2 * 3 - 6 / 2 =
    // 1 +   6   -   3   = 4
    let head: isize = 1;
    let tail = [(Add, 2), (Mul, 3), (Sub, 6), (Div, 2)];
    let out = climb(head, tail);
    assert_eq!(out, 4);
}
