use crate::{ports::Error, protocol::Cadence};
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use chrono_tz::Tz;

pub(crate) fn next(cadence: &Cadence, after: DateTime<Utc>) -> Result<DateTime<Utc>, Error> {
    match cadence {
        Cadence::Every { every_ms } if *every_ms > 0 => after
            .checked_add_signed(Duration::milliseconds(*every_ms))
            .ok_or(Error::Invalid),
        Cadence::Every { .. } => Err(Error::Invalid),
        Cadence::Cron {
            expression,
            timezone,
        } => {
            let fields = parse(expression)?;
            let zone: Tz = timezone
                .as_deref()
                .unwrap_or("UTC")
                .parse()
                .map_err(|_| Error::Invalid)?;
            let seconds = after
                .timestamp()
                .div_euclid(60)
                .checked_add(1)
                .and_then(|n| n.checked_mul(60))
                .ok_or(Error::Invalid)?;
            let mut cursor = DateTime::from_timestamp(seconds, 0).ok_or(Error::Invalid)?;
            for _ in 0..366 * 24 * 60 {
                let local = cursor.with_timezone(&zone);
                let values = [
                    local.minute(),
                    local.hour(),
                    local.day(),
                    local.month(),
                    local.weekday().num_days_from_sunday(),
                ];
                if fields
                    .iter()
                    .zip(values)
                    .all(|(mask, value)| mask & (1u64 << value) != 0)
                {
                    return Ok(cursor);
                }
                cursor = cursor
                    .checked_add_signed(Duration::minutes(1))
                    .ok_or(Error::Invalid)?;
            }
            Err(Error::Invalid)
        }
    }
}

fn parse(expression: &str) -> Result<[u64; 5], Error> {
    let parts: Vec<_> = expression.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(Error::Invalid);
    }
    let mut fields = [0; 5];
    for (index, (min, max)) in [(0, 59), (0, 23), (1, 31), (1, 12), (0, 6)]
        .into_iter()
        .enumerate()
    {
        for part in parts[index].split(',') {
            let (base, step) = match part.split_once('/') {
                Some((base, step)) => {
                    let parsed: u32 = step.parse().map_err(|_| Error::Invalid)?;
                    if parsed == 0 || parsed.to_string() != step {
                        return Err(Error::Invalid);
                    }
                    (base, parsed)
                }
                None => (part, 1),
            };
            let (start, end) = if base == "*" {
                (min, max)
            } else if let Some((start, end)) = base.split_once('-') {
                (number(start)?, number(end)?)
            } else {
                let value = number(base)?;
                (value, value)
            };
            if start < min || end > max || start > end {
                return Err(Error::Invalid);
            }
            let mut value = start;
            loop {
                fields[index] |= 1u64 << value;
                let Some(next) = value.checked_add(step) else {
                    break;
                };
                if next > end {
                    break;
                }
                value = next;
            }
        }
    }
    Ok(fields)
}
fn number(value: &str) -> Result<u32, Error> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid);
    }
    value.parse().map_err(|_| Error::Invalid)
}
#[cfg(test)]
mod tests;
