//! Inclusive numeric bounds for graph metrics.
//!
//! A range is `exact`, or one or two of `min` and `max`. `exact` is mutually
//! exclusive with the bounds, every value is a non-negative integer, and a
//! `min` above its `max` is rejected. A metric a package does not have — an
//! unreachable depth, an unreachable distance — is absent rather than zero,
//! which is why [`Range::check`] takes an option.

use toml::Value;

/// An inclusive range from a config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Range {
    /// Exactly one value.
    Exact(usize),
    /// At least `min`, at most `max`, and at least one of them is present.
    Bounds {
        /// The inclusive lower bound.
        min: Option<usize>,
        /// The inclusive upper bound.
        max: Option<usize>,
    },
}

impl Range {
    /// Parse a range table: `min`, `max` and `exact` keys, at least one.
    pub(crate) fn parse(value: &Value) -> Result<Range, String> {
        let table = value
            .as_table()
            .ok_or_else(|| format!("a range must be a table, found {value}"))?;
        let mut min = None;
        let mut max = None;
        let mut exact = None;
        for (key, value) in table {
            let number = value.as_integer().ok_or_else(|| {
                format!("range `{key}` must be a non-negative integer, found {value}")
            })?;
            let number = usize::try_from(number).map_err(|_| {
                format!("range `{key}` must be a non-negative integer, found {number}")
            })?;
            match key.as_str() {
                "min" => min = Some(number),
                "max" => max = Some(number),
                "exact" => exact = Some(number),
                other => return Err(format!("unknown range key `{other}`")),
            }
        }
        if let Some(exact) = exact {
            if min.is_some() || max.is_some() {
                return Err("`exact` cannot be combined with `min` or `max`".to_owned());
            }
            return Ok(Self::Exact(exact));
        }
        if min.is_none() && max.is_none() {
            return Err("a range needs at least one of `min`, `max` and `exact`".to_owned());
        }
        if let (Some(min), Some(max)) = (min, max)
            && min > max
        {
            return Err(format!("`min {min}` is greater than `max {max}`"));
        }
        Ok(Self::Bounds { min, max })
    }

    /// Whether `value` satisfies the range.
    ///
    /// An absent value passes a `min`-only range and fails `max` and `exact`,
    /// because "at least this deep" holds vacuously while a cap or an exact
    /// value does not.
    pub(crate) fn check(&self, value: Option<usize>) -> bool {
        match (self, value) {
            (Self::Exact(exact), Some(value)) => value == *exact,
            (Self::Exact(_), None) => false,
            (Self::Bounds { min, max }, Some(value)) => {
                min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
            }
            (Self::Bounds { max, .. }, None) => max.is_none(),
        }
    }

    /// Render the range in the contract spelling.
    pub(crate) fn render(&self) -> String {
        match self {
            Self::Exact(exact) => format!("exact {exact}"),
            Self::Bounds {
                min: Some(min),
                max: Some(max),
            } => format!("min {min}, max {max}"),
            Self::Bounds {
                min: Some(min),
                max: None,
            } => format!("min {min}"),
            Self::Bounds {
                min: None,
                max: Some(max),
            } => format!("max {max}"),
            Self::Bounds {
                min: None,
                max: None,
            } => unreachable!("a parsed range has a bound"),
        }
    }
}

#[cfg(test)]
impl Range {
    /// Exactly `value`.
    pub(crate) fn exact(value: usize) -> Self {
        Self::Exact(value)
    }

    /// At least `value`.
    pub(crate) fn min(value: usize) -> Self {
        Self::Bounds {
            min: Some(value),
            max: None,
        }
    }

    /// At most `value`.
    pub(crate) fn max(value: usize) -> Self {
        Self::Bounds {
            min: None,
            max: Some(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(source: &str) -> Range {
        let value = toml::from_str::<Value>(&format!("range = {source}"))
            .expect("valid TOML")
            .get("range")
            .cloned()
            .expect("a range");
        Range::parse(&value).expect("a valid range")
    }

    fn error(source: &str) -> String {
        let value = toml::from_str::<Value>(&format!("range = {source}"))
            .expect("valid TOML")
            .get("range")
            .cloned()
            .expect("a range");
        Range::parse(&value).expect_err("a range error")
    }

    #[test]
    fn bounds_are_inclusive() {
        let at_most_two = range("{ max = 2 }");
        assert!(at_most_two.check(Some(0)));
        assert!(at_most_two.check(Some(2)));
        assert!(!at_most_two.check(Some(3)));

        let at_least_one = range("{ min = 1 }");
        assert!(!at_least_one.check(Some(0)));
        assert!(at_least_one.check(Some(1)));

        let between = range("{ min = 1, max = 2 }");
        assert!(!between.check(Some(0)));
        assert!(between.check(Some(1)));
        assert!(between.check(Some(2)));
        assert!(!between.check(Some(3)));

        let exact = range("{ exact = 2 }");
        assert!(!exact.check(Some(1)));
        assert!(exact.check(Some(2)));
        assert!(!exact.check(Some(3)));
    }

    #[test]
    fn absent_values_pass_only_a_lower_bound() {
        assert!(range("{ min = 1 }").check(None));
        assert!(!range("{ max = 1 }").check(None));
        assert!(!range("{ exact = 0 }").check(None));
        assert!(!range("{ min = 1, max = 2 }").check(None));
    }

    #[test]
    fn ranges_render_their_bounds() {
        assert_eq!(range("{ exact = 2 }").render(), "exact 2");
        assert_eq!(range("{ min = 1 }").render(), "min 1");
        assert_eq!(range("{ max = 3 }").render(), "max 3");
        assert_eq!(range("{ min = 1, max = 3 }").render(), "min 1, max 3");
    }

    #[test]
    fn malformed_ranges_are_rejected() {
        assert!(error("{ min = 1, exact = 1 }").contains("`exact` cannot be combined"));
        assert!(error("{}").contains("at least one of"));
        assert!(error("{ min = -1 }").contains("non-negative integer"));
        assert!(error("{ max = 1.5 }").contains("non-negative integer"));
        assert!(error("{ mid = 1 }").contains("unknown range key `mid`"));
        assert!(error("1").contains("a range must be a table"));
        assert!(error("{ min = 3, max = 2 }").contains("`min 3` is greater than `max 2`"));
    }

    #[test]
    fn equal_bounds_are_a_single_value() {
        let same = range("{ min = 2, max = 2 }");
        assert!(same.check(Some(2)));
        assert_eq!(same.render(), "min 2, max 2");
    }
}
