use serde::{Deserialize, Deserializer};

use super::Mergeable;

pub(super) fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

pub(super) fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

pub(super) fn positive_integer<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    safe_integer(f64::deserialize(d)?)
}

fn safe_integer<E: serde::de::Error>(value: f64) -> Result<u64, E> {
    if value.is_finite() && (1.0..=9_007_199_254_740_991.0).contains(&value) && value.fract() == 0.0
    {
        // Formatting an already bounded integral value avoids lossy float-to-int casts.
        format!("{value:.0}").parse().map_err(E::custom)
    } else {
        Err(E::custom("expected a positive safe integer"))
    }
}

pub(super) fn optional_positive_integer<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<u64>, D::Error> {
    positive_integer(d).map(Some)
}

pub(super) fn nullable_positive_integer<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<u64>, D::Error> {
    Option::<f64>::deserialize(d)?.map(safe_integer).transpose()
}

pub(super) fn mergeable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Mergeable>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(Some(
        serde_json::from_value(value).unwrap_or(Mergeable::Unknown),
    ))
}
