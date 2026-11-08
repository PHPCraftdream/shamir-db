/// Stable runtime cell tags. Values occupy a 16-byte ABI slot.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CellTag {
    Unused = 0,
    Null = 1,
    Bool = 2,
    I32 = 3,
    I64 = 4,
    F64 = 5,
    String = 6,
    Bytes = 7,
    List = 8,
    Record = 9,
    Validation = 10,
    Batch = 11,
    Request = 12,
    Response = 13,
    DynamicMap = 14,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Cell {
    tag: CellTag,
    auxiliary: u32,
    payload: u64,
}

impl Cell {
    pub fn new(tag: CellTag, auxiliary: u32, payload: u64) -> Self {
        Self {
            tag,
            auxiliary,
            payload,
        }
    }

    pub fn tag(self) -> CellTag {
        self.tag
    }
    pub fn auxiliary(self) -> u32 {
        self.auxiliary
    }
    pub fn payload(self) -> u64 {
        self.payload
    }

    pub fn encode(self) -> [u8; 16] {
        let mut bytes = [0; 16];
        bytes[0..4].copy_from_slice(&(self.tag as u32).to_le_bytes());
        bytes[4..8].copy_from_slice(&self.auxiliary.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.payload.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: [u8; 16]) -> Option<Self> {
        let raw = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let tag = match raw {
            0 => CellTag::Unused,
            1 => CellTag::Null,
            2 => CellTag::Bool,
            3 => CellTag::I32,
            4 => CellTag::I64,
            5 => CellTag::F64,
            6 => CellTag::String,
            7 => CellTag::Bytes,
            8 => CellTag::List,
            9 => CellTag::Record,
            10 => CellTag::Validation,
            11 => CellTag::Batch,
            12 => CellTag::Request,
            13 => CellTag::Response,
            14 => CellTag::DynamicMap,
            _ => return None,
        };
        Some(Self {
            tag,
            auxiliary: u32::from_le_bytes(bytes[4..8].try_into().ok()?),
            payload: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        })
    }
}
