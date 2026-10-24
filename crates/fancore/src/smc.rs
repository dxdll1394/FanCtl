//! Minimal, dependency-free AppleSMC access for Apple Silicon Macs.
//!
//! Reading works as an unprivileged user; writing fan keys (`F0Tg`, `F0Md`)
//! requires root. Everything here is `unsafe` FFI into IOKit, wrapped in a
//! small safe API. The packet layout mirrors `AppleSMCUserClient` and is
//! asserted to be 80 bytes on 64-bit macOS.

use std::ffi::c_void;
use std::fmt;

use crate::types::FanMode;

pub const KERNEL_INDEX_SMC: u32 = 2;
pub const SMC_CMD_READ_BYTES: u8 = 5;
pub const SMC_CMD_WRITE_BYTES: u8 = 6;
pub const SMC_CMD_READ_KEYINFO: u8 = 9;
pub const SMC_CMD_READ_KEYBYINDEX: u8 = 8;

pub const KIORETURN_SUCCESS: i32 = 0;
pub const KIORETURN_NOT_PRIVILEGED: i32 = -536_870_207; // 0xe00002c1
pub const KIORETURN_BAD_ARGUMENT: i32 = -536_870_206;
pub const KIORETURN_NOT_FOUND: i32 = -536_870_165;

type KernReturn = i32;
type IoService = u32;
type IoConnect = u32;
type MachPort = u32;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const std::os::raw::c_char) -> *const c_void;
    fn IOServiceGetMatchingService(main_port: MachPort, matching: *const c_void) -> IoService;
    fn IOServiceOpen(
        service: IoService,
        owning_task: MachPort,
        kind: u32,
        connection: *mut IoConnect,
    ) -> KernReturn;
    fn IOServiceClose(connection: IoConnect) -> KernReturn;
    fn IOObjectRelease(object: u32) -> KernReturn;
    fn IOConnectCallStructMethod(
        connection: IoConnect,
        selector: u32,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
        output_size: *mut usize,
    ) -> KernReturn;
}

extern "C" {
    static mach_task_self_: MachPort;
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vers {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PLimit {
    version: u16,
    length: u16,
    cpu_plimit: u32,
    gpu_plimit: u32,
    mem_plimit: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KeyInfo {
    data_size: u32,
    data_type: u32,
    data_attributes: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KeyData {
    key: u32,
    vers: Vers,
    p_limit_data: PLimit,
    key_info: KeyInfo,
    result: u8,
    status: u8,
    data8: u8,
    data32: u32,
    bytes: [u8; 32],
}

#[derive(Debug, Clone)]
pub enum SmcError {
    Open(String),
    Io { code: KernReturn, what: &'static str },
    Key { key: String, code: u32, what: &'static str },
    UnsupportedType { key: String, data_type: String },
    Size { key: String, expected: usize, got: usize },
}

impl fmt::Display for SmcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SmcError::Open(msg) => write!(f, "无法打开 AppleSMC：{msg}"),
            SmcError::Io { code, what } => {
                write!(f, "{what} 失败：0x{:08x}", *code as u32)
            }
            SmcError::Key { key, code, what } => {
                let hint = match *code as i32 {
                    KIORETURN_NOT_PRIVILEGED => "（需要 root 权限）",
                    KIORETURN_BAD_ARGUMENT => "（请求格式错误）",
                    KIORETURN_NOT_FOUND => "（本机没有这个键）",
                    _ => "",
                };
                write!(f, "SMC 键 {key} {what} 失败：0x{code:08x}{hint}")
            }
            SmcError::UnsupportedType { key, data_type } => {
                write!(f, "SMC 键 {key} 的类型 {data_type:?} 暂不支持")
            }
            SmcError::Size { key, expected, got } => {
                write!(f, "SMC 键 {key} 需要 {expected} 字节，实际 {got} 字节")
            }
        }
    }
}

impl std::error::Error for SmcError {}

fn fourcc(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if bytes.len() != 4 {
        return None;
    }
    Some(
        ((bytes[0] as u32) << 24)
            | ((bytes[1] as u32) << 16)
            | ((bytes[2] as u32) << 8)
            | (bytes[3] as u32),
    )
}

fn fourcc_to_string(key: u32) -> String {
    let b = key.to_be_bytes();
    b.iter().map(|c| if c.is_ascii_graphic() || *c == b' ' { *c as char } else { '.' }).collect()
}

pub struct Smc {
    conn: IoConnect,
}

// AppleSMC user client calls are serialized by IOKit for a given connection;
// we guard all access behind a Mutex in the daemon.
unsafe impl Send for Smc {}

impl Smc {
    pub fn open() -> Result<Self, SmcError> {
        unsafe {
            let matching = IOServiceMatching(c"AppleSMC".as_ptr());
            if matching.is_null() {
                return Err(SmcError::Open("IOServiceMatching 返回空".into()));
            }
            let service = IOServiceGetMatchingService(0, matching);
            if service == 0 {
                return Err(SmcError::Open("找不到 AppleSMC 服务".into()));
            }
            let mut conn: IoConnect = 0;
            let kr = IOServiceOpen(service, mach_task_self_, 0, &mut conn);
            IOObjectRelease(service);
            if kr != KIORETURN_SUCCESS {
                return Err(SmcError::Io { code: kr, what: "IOServiceOpen(AppleSMC)" });
            }
            Ok(Self { conn })
        }
    }

    fn call(&self, packet: &KeyData) -> Result<KeyData, SmcError> {
        let mut out = KeyData::default();
        let mut size = std::mem::size_of::<KeyData>();
        let input = packet as *const KeyData as *const c_void;
        let output = &mut out as *mut KeyData as *mut c_void;
        let kr = unsafe {
            IOConnectCallStructMethod(
                self.conn,
                KERNEL_INDEX_SMC,
                input,
                std::mem::size_of::<KeyData>(),
                output,
                &mut size,
            )
        };
        if kr != KIORETURN_SUCCESS {
            return Err(SmcError::Io { code: kr, what: "IOConnectCallStructMethod" });
        }
        Ok(out)
    }

    pub fn key_info(&self, key: &str) -> Result<(u32, String), SmcError> {
        let code = fourcc(key).ok_or_else(|| SmcError::Key {
            key: key.into(),
            code: 0,
            what: "非法键名",
        })?;
        let pkt = KeyData { key: code, data8: SMC_CMD_READ_KEYINFO, ..KeyData::default() };
        let out = self.call(&pkt)?;
        if out.result != 0 {
            return Err(SmcError::Key {
                key: key.into(),
                code: out.result as u32,
                what: "读取键信息",
            });
        }
        let data_type = fourcc_to_string(out.key_info.data_type);
        Ok((out.key_info.data_size, data_type))
    }

    pub fn read_raw(&self, key: &str) -> Result<(String, Vec<u8>), SmcError> {
        let (size, data_type) = self.key_info(key)?;
        let code = fourcc(key).unwrap();
        let pkt = KeyData {
            key: code,
            data8: SMC_CMD_READ_BYTES,
            key_info: KeyInfo { data_size: size, ..KeyInfo::default() },
            ..KeyData::default()
        };
        let out = self.call(&pkt)?;
        if out.result != 0 {
            return Err(SmcError::Key {
                key: key.into(),
                code: out.result as u32,
                what: "读取数据",
            });
        }
        let n = (size as usize).min(32);
        Ok((data_type, out.bytes[..n].to_vec()))
    }

    pub fn write_raw(&self, key: &str, payload: &[u8]) -> Result<(), SmcError> {
        let (size, _) = self.key_info(key)?;
        if payload.len() != size as usize {
            return Err(SmcError::Size {
                key: key.into(),
                expected: size as usize,
                got: payload.len(),
            });
        }
        let code = fourcc(key).unwrap();
        let mut bytes = [0u8; 32];
        for (i, b) in payload.iter().enumerate().take(32) {
            bytes[i] = *b;
        }
        let pkt = KeyData {
            key: code,
            data8: SMC_CMD_WRITE_BYTES,
            key_info: KeyInfo { data_size: size, ..KeyInfo::default() },
            bytes,
            ..KeyData::default()
        };
        let out = self.call(&pkt)?;
        if out.result != 0 {
            return Err(SmcError::Key {
                key: key.into(),
                code: out.result as u32,
                what: "写入数据",
            });
        }
        Ok(())
    }

    /// Read a numeric key, supporting the encodings used by fan and temperature keys.
    pub fn read_f64(&self, key: &str) -> Result<f64, SmcError> {
        let (data_type, raw) = self.read_raw(key)?;
        decode(&data_type, &raw)
            .ok_or_else(|| SmcError::UnsupportedType { key: key.into(), data_type })
    }

    /// Write a numeric key, preserving the key's native encoding.
    pub fn write_f64(&self, key: &str, value: f64) -> Result<(), SmcError> {
        let (_, data_type) = self.key_info(key)?;
        let payload = encode(&data_type, value)
            .ok_or_else(|| SmcError::UnsupportedType { key: key.into(), data_type })?;
        self.write_raw(key, &payload)
    }

    pub fn key_by_index(&self, index: u32) -> Result<String, SmcError> {
        let pkt = KeyData { data8: SMC_CMD_READ_KEYBYINDEX, data32: index, ..KeyData::default() };
        let out = self.call(&pkt)?;
        if out.result != 0 {
            return Err(SmcError::Key {
                key: format!("#{index}"),
                code: out.result as u32,
                what: "按索引读取键名",
            });
        }
        Ok(fourcc_to_string(out.key))
    }

    /// Enumerate every SMC key name (1369 keys on an M4 mini, ~0.4 s).
    pub fn all_keys(&self) -> Vec<String> {
        let mut keys = Vec::with_capacity(2048);
        for i in 0..4096u32 {
            match self.key_by_index(i) {
                Ok(k) if !k.trim().is_empty() => keys.push(k),
                _ => break,
            }
        }
        keys
    }

    /// All readable temperature keys, `(key, value)`.
    pub fn read_temperatures(&self, keys: &[String]) -> Vec<(String, f64)> {
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if let Ok(v) = self.read_f64(key) {
                if v.is_finite() && (-40.0..=150.0).contains(&v) {
                    out.push((key.clone(), v));
                }
            }
        }
        out
    }

    pub fn read_fan(&self, index: u8) -> Result<crate::types::FanReading, SmcError> {
        let p = format!("F{index}");
        let actual = self.read_f64(&format!("{p}Ac"))?;
        let target = self.read_f64(&format!("{p}Tg")).unwrap_or(actual);
        let min = self.read_f64(&format!("{p}Mn")).unwrap_or(0.0);
        let max = self.read_f64(&format!("{p}Mx")).unwrap_or(0.0);
        let mode = self
            .read_f64(&format!("{p}Md"))
            .map(|v| FanMode::from_raw(v as i32))
            .unwrap_or(FanMode::Unknown);
        Ok(crate::types::FanReading {
            index,
            actual_rpm: actual,
            target_rpm: target,
            min_rpm: min,
            max_rpm: max,
            mode,
        })
    }

    pub fn fan_count(&self) -> u8 {
        self.read_f64("FNum").unwrap_or(0.0).clamp(0.0, 8.0) as u8
    }

    pub fn read_fans(&self) -> Vec<crate::types::FanReading> {
        (0..self.fan_count()).filter_map(|i| self.read_fan(i).ok()).collect()
    }

    pub fn set_fan_rpm(&self, index: u8, rpm: f64) -> Result<(), SmcError> {
        let prefix = format!("F{index}");
        let min = self.read_f64(&format!("{prefix}Mn")).unwrap_or(0.0);
        let max = self.read_f64(&format!("{prefix}Mx")).unwrap_or(6000.0);
        let rpm = rpm.clamp(min, max);
        // Manual mode must be set before the target, otherwise the SMC ignores it.
        self.write_f64(&format!("{prefix}Md"), 1.0)?;
        self.write_f64(&format!("{prefix}Tg"), rpm)?;
        Ok(())
    }

    pub fn reset_fan(&self, index: u8) -> Result<(), SmcError> {
        self.write_f64(&format!("F{index}Md"), 0.0)
    }

    pub fn reset_all(&self) -> Result<(), SmcError> {
        for i in 0..self.fan_count() {
            self.reset_fan(i)?;
        }
        Ok(())
    }
}

impl Drop for Smc {
    fn drop(&mut self) {
        if self.conn != 0 {
            unsafe {
                IOServiceClose(self.conn);
            }
            self.conn = 0;
        }
    }
}

fn decode(data_type: &str, raw: &[u8]) -> Option<f64> {
    match (data_type, raw.len()) {
        ("flt ", 4) => Some(f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as f64),
        ("fpe2", 2) => Some(u16::from_be_bytes([raw[0], raw[1]]) as f64 / 4.0),
        ("ui8 ", 1) => Some(raw[0] as f64),
        ("ui8", 1) => Some(raw[0] as f64),
        ("ui16", 2) => Some(u16::from_be_bytes([raw[0], raw[1]]) as f64),
        ("ui32", 4) => Some(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as f64),
        ("si8 ", 1) => Some(raw[0] as i8 as f64),
        ("si16", 2) => Some(i16::from_be_bytes([raw[0], raw[1]]) as f64),
        ("si32", 4) => Some(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as f64),
        ("sp78", 2) => Some(i16::from_be_bytes([raw[0], raw[1]]) as f64 / 256.0),
        ("sp87", 2) => Some(i16::from_be_bytes([raw[0], raw[1]]) as f64 / 128.0),
        _ => None,
    }
}

fn encode(data_type: &str, value: f64) -> Option<Vec<u8>> {
    match data_type {
        "flt " => Some((value as f32).to_le_bytes().to_vec()),
        "fpe2" => Some(((value * 4.0).round() as u16).to_be_bytes().to_vec()),
        "ui8 " | "ui8" => Some(vec![value.round().clamp(0.0, 255.0) as u8]),
        "ui16" => Some((value.round() as u16).to_be_bytes().to_vec()),
        "ui32" => Some((value.round() as u32).to_be_bytes().to_vec()),
        "si8 " => Some(vec![(value.round() as i8) as u8]),
        "si16" => Some((value.round() as i16).to_be_bytes().to_vec()),
        "si32" => Some((value.round() as i32).to_be_bytes().to_vec()),
        "sp78" => Some(((value * 256.0).round() as i16).to_be_bytes().to_vec()),
        "sp87" => Some(((value * 128.0).round() as i16).to_be_bytes().to_vec()),
        _ => None,
    }
}

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_layout_matches_apple_smc() {
        assert_eq!(std::mem::size_of::<KeyData>(), 80);
        assert_eq!(std::mem::size_of::<KeyInfo>(), 12);
    }

    #[test]
    fn fourcc_roundtrip() {
        assert_eq!(fourcc_to_string(fourcc("F0Tg").unwrap()), "F0Tg");
        assert!(fourcc("F0T").is_none());
    }

    #[test]
    fn float_decoding() {
        assert_eq!(decode("flt ", &1000.0f32.to_le_bytes()), Some(1000.0));
        assert_eq!(decode("fpe2", &(2000u16 * 4).to_be_bytes()), Some(2000.0));
        assert_eq!(decode("ui8 ", &[1]), Some(1.0));
    }
}
