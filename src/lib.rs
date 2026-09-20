use std::{fs, path::PathBuf};

mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum Error {
        #[error(transparent)]
        IOError(std::io::Error),

        #[error("a CRC failure occurred while reading from the sensor")]
        CRCFailure,

        #[error("an unknown error occurred while reading from the sensor")]
        Unknown,
    }

    impl Error {
        pub fn is_retryable(&self) -> bool {
            match self {
                &Self::CRCFailure => true,
                &Self::IOError(_) => false,
                &Self::Unknown => false,
            }
        }
    }
}

pub trait SensorValueSource {
    fn read(&self) -> Result<String, std::io::Error>;
}

struct FileSensorValueSource {
    path: PathBuf,
}

impl SensorValueSource for FileSensorValueSource {
    fn read(&self) -> Result<String, std::io::Error> {
        fs::read_to_string(self.path.as_path())
    }
}

impl From<std::io::Error> for error::Error {
    fn from(e: std::io::Error) -> Self {
        Self::IOError(e)
    }
}

pub trait W1Therm {
    const SCALE: u32;

    fn read_u16(&self) -> Result<u16, error::Error>;

    fn read_f32(&self) -> Result<f32, error::Error> {
        let value = self.read_u16()?;
        Ok(value as f32 / u32::pow(10, Self::SCALE) as f32)
    }

    fn read_f64(&self) -> Result<f64, error::Error> {
        let value = self.read_u16()?;
        Ok(value as f64 / u32::pow(10, Self::SCALE) as f64)
    }

    #[cfg(feature = "rust_decimal")]
    fn read_dec(&self) -> Result<rust_decimal::Decimal, error::Error> {
        let value = self.read_u16()?;
        Ok(rust_decimal::Decimal::from_i128_with_scale(
            value as i128,
            Self::SCALE,
        ))
    }
}

pub struct DS18B20 {
    pub(crate) source: Box<dyn SensorValueSource>,
}

unsafe impl Send for DS18B20 {}
unsafe impl Sync for DS18B20 {}

impl DS18B20 {
    pub fn new(path: &str) -> Self {
        let mut path = PathBuf::from(path);
        if !path.ends_with("w1_slave") {
            path.push("w1_slave");
        }

        DS18B20 {
            source: Box::new(FileSensorValueSource { path }),
        }
    }
}

impl W1Therm for DS18B20 {
    const SCALE: u32 = 3;

    fn read_u16(&self) -> Result<u16, error::Error> {
        let binding = self.source.read()?;
        let contents: Vec<_> = binding.split_ascii_whitespace().collect();

        if contents[11] != "YES" {
            return Err(error::Error::CRCFailure);
        }

        contents[21]
            .strip_prefix("t=")
            .and_then(|w| w.parse::<u16>().ok())
            .ok_or(error::Error::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    struct StringSensorValueSource<'a> {
        pub string: &'a str,
    }

    impl SensorValueSource for StringSensorValueSource<'_> {
        fn read(&self) -> Result<String, std::io::Error> {
            Ok(self.string.to_string())
        }
    }

    #[test]
    fn ds18b20() {
        let ds18b20_crc_failure = DS18B20 {
            source: Box::new(StringSensorValueSource {
                string: indoc! {r#"
                ab 01 55 00 7f ff 0c 10 1d : crc=1d NO
                ab 01 55 00 7f ff 0c 10 1d t=26687
            "#},
            }),
        };
        let read_result = ds18b20_crc_failure.read_u16();
        assert!(read_result.is_err());
        assert!(read_result.unwrap_err().is_retryable());

        let ds18b20_ok = DS18B20 {
            source: Box::new(StringSensorValueSource {
                string: indoc! {r#"
                ab 01 55 00 7f ff 0c 10 1d : crc=1d YES
                ab 01 55 00 7f ff 0c 10 1d t=26687
            "#},
            }),
        };
        let read_result = ds18b20_ok.read_u16();
        assert!(read_result.is_ok());
        assert_eq!(read_result.unwrap(), 26687);

        assert_eq!(ds18b20_ok.read_f32().unwrap(), 26.687);
        assert_eq!(ds18b20_ok.read_f64().unwrap(), 26.687);

        #[cfg(feature = "rust_decimal")]
        assert_eq!(ds18b20_ok.read_dec().unwrap(), rust_decimal::dec!(26.687));
    }
}
