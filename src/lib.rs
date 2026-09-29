//! Small wrapper around reading from 1-Wire temperature sensors under Linux.
//!
//! The goal was to split some work out from an internal project so it could be helpful
//! to others, as well as get some experience publishing a crate.

#![warn(missing_docs)]

use std::{fs, path::PathBuf};

/// Error structures for `w1-therm`
pub mod error {
    use thiserror::Error;

    /// Error.
    #[derive(Debug, Error)]
    pub enum Error {
        /// An io::Error occurred while reading from the sensor.
        #[error(transparent)]
        IOError(std::io::Error),

        /// A num::ParseIntError occured while parsing the sensor value.
        #[error(transparent)]
        ParseIntError(std::num::ParseIntError),

        /// The CRC check for the value failed; the value should be read again.
        #[error("a CRC failure occurred while reading from the sensor")]
        CRCFailure,

        /// Encountered a badly formatted string while reading from the w1-therm file.
        #[error("unexpected format error: {0}")]
        FormattingError(String),
    }

    impl Error {
        /// Should the function which caused the error be retried?
        pub fn is_retryable(&self) -> bool {
            match self {
                &Self::CRCFailure => true,
                &Self::IOError(_) => false,
                &Self::ParseIntError(_) => false,
                &Self::FormattingError(_) => false,
            }
        }
    }
    impl From<std::io::Error> for Error {
        fn from(e: std::io::Error) -> Self {
            Self::IOError(e)
        }
    }

    impl From<std::num::ParseIntError> for Error {
        fn from(e: std::num::ParseIntError) -> Self {
            Self::ParseIntError(e)
        }
    }
}

trait SensorValueSource {
    fn read(&self) -> Result<String, std::io::Error>;
}

struct StringSensorValueSource<'a> {
    pub string: &'a str,
}

impl SensorValueSource for StringSensorValueSource<'_> {
    fn read(&self) -> Result<String, std::io::Error> {
        Ok(self.string.to_string())
    }
}

struct FileSensorValueSource {
    path: PathBuf,
}

impl SensorValueSource for FileSensorValueSource {
    fn read(&self) -> Result<String, std::io::Error> {
        fs::read_to_string(self.path.as_path())
    }
}

/// A value read from a w1_therm compatible sensor.
pub trait W1Therm {
    /// The scale of the integer read from the sensor.
    ///
    /// The actual value the sensor provides is: raw / 10^SCALE
    const SCALE: u32;

    /// Read the raw, unscaled value of the sensor.
    ///
    /// This method is used by the trait to implement the other methods, and
    /// is probably not what you want to be using.
    fn read_raw(&self) -> Result<u16, error::Error>;

    /// Value of the sensor as an f32.
    fn read_f32(&self) -> Result<f32, error::Error> {
        let value = self.read_raw()?;
        Ok(value as f32 / u32::pow(10, Self::SCALE) as f32)
    }

    /// Value of the sensor as an f64.
    fn read_f64(&self) -> Result<f64, error::Error> {
        let value = self.read_raw()?;
        Ok(value as f64 / u32::pow(10, Self::SCALE) as f64)
    }

    /// Value of the sensor as a rust_decimal, using `from_i128_with_scale`.
    #[cfg(feature = "rust_decimal")]
    fn read_dec(&self) -> Result<rust_decimal::Decimal, error::Error> {
        let value = self.read_raw()?;
        Ok(rust_decimal::Decimal::from_i128_with_scale(
            value as i128,
            Self::SCALE,
        ))
    }
}

/// Implementation for the DS18B20.
pub struct DS18B20 {
    pub(crate) source: Box<dyn SensorValueSource>,
}

unsafe impl Send for DS18B20 {}
unsafe impl Sync for DS18B20 {}

impl DS18B20 {
    // pub fn from_id(id: &str) -> Self {
    //     let mut path = PathBuf::from("/sys/bus/w1/devices/");
    //     path.push(format!("28-{id}"));
    //     Self::new(path.to_str())
    // }

    /// Create with the path to the device, e.g. `/sys/bus/w1/devices/28-2403000db0b9`
    pub fn new(path: &str) -> Self {
        let mut path = PathBuf::from(path);
        if !path.ends_with("w1_slave") {
            path.push("w1_slave");
        }

        DS18B20 {
            source: Box::new(FileSensorValueSource { path }),
        }
    }

    /// Create a DS18B20 which always returns a reading of 0.
    pub fn zero() -> Self {
        DS18B20 {
            source: Box::new(StringSensorValueSource {
                string: r#"
                ab 01 55 00 7f ff 0c 10 1d : crc=1d YES
                ab 01 55 00 7f ff 0c 10 1d t=0
            "#,
            }),
        }
    }
}

impl W1Therm for DS18B20 {
    const SCALE: u32 = 3;

    fn read_raw(&self) -> Result<u16, error::Error> {
        let binding = self.source.read()?;
        let contents: Vec<_> = binding.split_ascii_whitespace().collect();

        if contents[11] != "YES" {
            return Err(error::Error::CRCFailure);
        }

        contents[21]
            .strip_prefix("t=")
            .ok_or(error::Error::FormattingError(
                "token 't=' not found while reading value".to_string(),
            ))
            .and_then(|w| Ok(w.parse::<u16>()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ds18b20() {
        let ds18b20_crc_failure = DS18B20 {
            source: Box::new(StringSensorValueSource {
                string: r#"
                ab 01 55 00 7f ff 0c 10 1d : crc=1d NO
                ab 01 55 00 7f ff 0c 10 1d t=26687
            "#,
            }),
        };
        let read_result = ds18b20_crc_failure.read_raw();
        assert!(read_result.is_err());
        assert!(read_result.unwrap_err().is_retryable());

        let ds18b20_ok = DS18B20 {
            source: Box::new(StringSensorValueSource {
                string: r#"
                ab 01 55 00 7f ff 0c 10 1d : crc=1d YES
                ab 01 55 00 7f ff 0c 10 1d t=26687
            "#,
            }),
        };
        let read_result = ds18b20_ok.read_raw();
        assert!(read_result.is_ok());
        assert_eq!(read_result.unwrap(), 26687);

        assert_eq!(ds18b20_ok.read_f32().unwrap(), 26.687);
        assert_eq!(ds18b20_ok.read_f64().unwrap(), 26.687);

        #[cfg(feature = "rust_decimal")]
        assert_eq!(ds18b20_ok.read_dec().unwrap(), rust_decimal::dec!(26.687));
    }

    #[test]
    fn ds18b20_zero() {
        let ds18b20_zero = DS18B20::zero();
        let read_result = ds18b20_zero.read_raw();
        assert!(read_result.is_ok());
        assert_eq!(read_result.unwrap(), 0);

        assert_eq!(ds18b20_zero.read_f32().unwrap(), 0.0);
        assert_eq!(ds18b20_zero.read_f64().unwrap(), 0.0);

        #[cfg(feature = "rust_decimal")]
        assert_eq!(ds18b20_zero.read_dec().unwrap(), rust_decimal::dec!(0.0));
    }
}
