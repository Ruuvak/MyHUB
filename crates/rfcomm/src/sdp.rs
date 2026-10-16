//! Minimal SDP (Service Discovery Protocol) client logic: encode one
//! `ServiceSearchAttributeRequest` and read the RFCOMM channel out of the answer.
//!
//! Pure bytes in, bytes out, no I/O. The Linux backend sends the request over an L2CAP
//! socket; the parser never panics on short or hostile input.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// `SDP_ServiceSearchAttributeRequest`.
const PDU_SEARCH_ATTR_REQUEST: u8 = 0x06;
/// `SDP_ServiceSearchAttributeResponse`.
const PDU_SEARCH_ATTR_RESPONSE: u8 = 0x07;
/// `SDP_ErrorResponse`.
const PDU_ERROR_RESPONSE: u8 = 0x01;
/// ProtocolDescriptorList attribute id.
const ATTR_PROTOCOL_DESCRIPTOR_LIST: u16 = 0x0004;
/// RFCOMM protocol UUID (16 bit short form).
const UUID_RFCOMM: u16 = 0x0003;
/// Longest continuation state the specification allows.
const MAX_CONTINUATION: usize = 16;

const TYPE_UINT: u8 = 1;
const TYPE_UUID: u8 = 3;
const TYPE_SEQUENCE: u8 = 6;
const TYPE_ALTERNATIVE: u8 = 7;
/// RFCOMM as a full 128 bit UUID (Bluetooth base UUID with short value 3).
const UUID_RFCOMM_128: [u8; 16] = [
    0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB,
];

/// Why an SDP answer could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdpError {
    /// The bytes ended before the structure did.
    Truncated,
    /// The bytes do not follow the SDP layout.
    Malformed,
    /// The peer answered with an SDP error PDU (the error code).
    ErrorResponse(u16),
    /// The answer belongs to another transaction.
    WrongTransaction,
}

/// One decoded response PDU.
#[derive(Debug, PartialEq, Eq)]
pub struct Response<'a> {
    /// The attribute list bytes carried by this PDU (one fragment).
    pub data: &'a [u8],
    /// Continuation state to echo in the next request; empty when the answer is complete.
    pub continuation: &'a [u8],
}

/// Encode a `ServiceSearchAttributeRequest` for one 128 bit service UUID (big endian
/// bytes) asking for the ProtocolDescriptorList attribute only. `continuation` is empty
/// for the first request, else the state from the previous response.
pub fn encode_request(transaction: u16, uuid: &[u8; 16], continuation: &[u8]) -> Vec<u8> {
    let mut params = Vec::with_capacity(32 + continuation.len());
    // ServiceSearchPattern: sequence of one UUID128 (type 3, size index 4).
    params.extend_from_slice(&[0x35, 17, 0x1C]);
    params.extend_from_slice(uuid);
    // MaximumAttributeByteCount.
    params.extend_from_slice(&0xFFFFu16.to_be_bytes());
    // AttributeIDList: sequence of one uint16.
    params.extend_from_slice(&[0x35, 0x03, 0x09]);
    params.extend_from_slice(&ATTR_PROTOCOL_DESCRIPTOR_LIST.to_be_bytes());
    // ContinuationState.
    params.push(continuation.len().min(MAX_CONTINUATION) as u8);
    params.extend_from_slice(&continuation[..continuation.len().min(MAX_CONTINUATION)]);

    let mut out = Vec::with_capacity(5 + params.len());
    out.push(PDU_SEARCH_ATTR_REQUEST);
    out.extend_from_slice(&transaction.to_be_bytes());
    out.extend_from_slice(&(params.len() as u16).to_be_bytes());
    out.extend_from_slice(&params);
    out
}

fn be16(bytes: &[u8]) -> Result<u16, SdpError> {
    match bytes {
        [a, b, ..] => Ok(u16::from_be_bytes([*a, *b])),
        _ => Err(SdpError::Truncated),
    }
}

/// Decode one response PDU received for `transaction`.
pub fn parse_response(transaction: u16, pdu: &[u8]) -> Result<Response<'_>, SdpError> {
    let [id, t1, t2, p1, p2, rest @ ..] = pdu else {
        return Err(SdpError::Truncated);
    };
    if u16::from_be_bytes([*t1, *t2]) != transaction {
        return Err(SdpError::WrongTransaction);
    }
    let plen = usize::from(u16::from_be_bytes([*p1, *p2]));
    let params = rest.get(..plen).ok_or(SdpError::Truncated)?;
    match *id {
        PDU_ERROR_RESPONSE => Err(SdpError::ErrorResponse(be16(params)?)),
        PDU_SEARCH_ATTR_RESPONSE => {
            let count = usize::from(be16(params)?);
            let body = &params[2..];
            let data = body.get(..count).ok_or(SdpError::Truncated)?;
            let tail = &body[count..];
            let (&clen, state) = tail.split_first().ok_or(SdpError::Truncated)?;
            let clen = usize::from(clen);
            if clen > MAX_CONTINUATION {
                return Err(SdpError::Malformed);
            }
            let continuation = state.get(..clen).ok_or(SdpError::Truncated)?;
            Ok(Response { data, continuation })
        }
        _ => Err(SdpError::Malformed),
    }
}

/// One data element: its type descriptor and payload bytes.
#[derive(Debug, Clone, Copy)]
struct Element<'a> {
    kind: u8,
    payload: &'a [u8],
}

/// Read one element from the front of `bytes`; returns it and the bytes after it.
fn read_element(bytes: &[u8]) -> Result<(Element<'_>, &[u8]), SdpError> {
    let (&head, rest) = bytes.split_first().ok_or(SdpError::Truncated)?;
    let kind = head >> 3;
    let size_index = head & 7;
    let (len, rest) = match size_index {
        0 if kind == 0 => (0, rest),
        0..=4 => (1usize << size_index, rest),
        5 => {
            let (&n, rest) = rest.split_first().ok_or(SdpError::Truncated)?;
            (usize::from(n), rest)
        }
        6 => (usize::from(be16(rest)?), &rest[2..]),
        _ => {
            let b = rest.get(..4).ok_or(SdpError::Truncated)?;
            let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
            (
                usize::try_from(n).map_err(|_| SdpError::Malformed)?,
                &rest[4..],
            )
        }
    };
    let payload = rest.get(..len).ok_or(SdpError::Truncated)?;
    Ok((Element { kind, payload }, &rest[len..]))
}

/// True for the UUID element forms that name RFCOMM (16, 32 and 128 bit).
fn is_rfcomm_uuid(e: &Element<'_>) -> bool {
    e.kind == TYPE_UUID
        && (e.payload == UUID_RFCOMM.to_be_bytes()
            || e.payload == u32::from(UUID_RFCOMM).to_be_bytes()
            || e.payload == UUID_RFCOMM_128)
}

/// The children of a sequence (or alternative) element.
fn children(seq: Element<'_>) -> Result<Vec<Element<'_>>, SdpError> {
    if seq.kind != TYPE_SEQUENCE && seq.kind != TYPE_ALTERNATIVE {
        return Err(SdpError::Malformed);
    }
    let mut out = Vec::new();
    let mut rest = seq.payload;
    while !rest.is_empty() {
        let (e, next) = read_element(rest)?;
        out.push(e);
        rest = next;
    }
    Ok(out)
}

/// The RFCOMM channel inside one ProtocolDescriptorList value, if it names RFCOMM.
fn channel_in_protocols(list: Element<'_>) -> Result<Option<u8>, SdpError> {
    for protocol in children(list)? {
        if protocol.kind != TYPE_SEQUENCE && protocol.kind != TYPE_ALTERNATIVE {
            continue;
        }
        let items = children(protocol)?;
        let is_rfcomm = items.first().is_some_and(is_rfcomm_uuid);
        if is_rfcomm
            && let Some(p) = items.get(1)
            && p.kind == TYPE_UINT
            && let [channel] = p.payload
        {
            // RFCOMM server channels are 1..=30.
            if !(1..=30).contains(channel) {
                return Err(SdpError::Malformed);
            }
            return Ok(Some(*channel));
        }
    }
    Ok(None)
}

/// Find the RFCOMM channel in the concatenated attribute lists of all response
/// fragments. `Ok(None)` when no record carries an RFCOMM protocol entry.
pub fn rfcomm_channel(attribute_lists: &[u8]) -> Result<Option<u8>, SdpError> {
    let (outer, _) = read_element(attribute_lists)?;
    for record in children(outer)? {
        let fields = children(record)?;
        for pair in fields.chunks(2) {
            let [id, value] = pair else {
                return Err(SdpError::Malformed);
            };
            let is_protocols =
                id.kind == TYPE_UINT && id.payload == ATTR_PROTOCOL_DESCRIPTOR_LIST.to_be_bytes();
            if is_protocols && let Some(ch) = channel_in_protocols(*value)? {
                return Ok(Some(ch));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 956C7B26-D49A-4BA8-B03F-B17D393CB6E2, big endian.
    const MDR: [u8; 16] = [
        0x95, 0x6C, 0x7B, 0x26, 0xD4, 0x9A, 0x4B, 0xA8, 0xB0, 0x3F, 0xB1, 0x7D, 0x39, 0x3C, 0xB6,
        0xE2,
    ];

    fn des(body: &[u8]) -> Vec<u8> {
        let mut v = vec![0x35, body.len() as u8];
        v.extend_from_slice(body);
        v
    }

    /// A one-record attribute list whose ProtocolDescriptorList is L2CAP plus `rfcomm`.
    fn lists(rfcomm: Option<u8>) -> Vec<u8> {
        let mut protocols = des(&[0x19, 0x01, 0x00]);
        if let Some(ch) = rfcomm {
            protocols.extend(des(&[0x19, 0x00, 0x03, 0x08, ch]));
        }
        let mut record = vec![0x09, 0x00, 0x04];
        record.extend(des(&protocols));
        des(&des(&record))
    }

    fn response(transaction: u16, data: &[u8], continuation: &[u8]) -> Vec<u8> {
        let mut params = (data.len() as u16).to_be_bytes().to_vec();
        params.extend_from_slice(data);
        params.push(continuation.len() as u8);
        params.extend_from_slice(continuation);
        let mut out = vec![PDU_SEARCH_ATTR_RESPONSE];
        out.extend_from_slice(&transaction.to_be_bytes());
        out.extend_from_slice(&(params.len() as u16).to_be_bytes());
        out.extend(params);
        out
    }

    #[test]
    fn request_encodes_the_service_pattern_and_attribute_range() {
        let mut want = vec![0x06, 0x00, 0x01, 0x00, 0x1B, 0x35, 0x11, 0x1C];
        want.extend_from_slice(&MDR);
        want.extend_from_slice(&[0xFF, 0xFF, 0x35, 0x03, 0x09, 0x00, 0x04, 0x00]);
        assert_eq!(encode_request(1, &MDR, &[]), want);
        let with_state = encode_request(2, &MDR, &[0xAA, 0xBB]);
        assert_eq!(&with_state[with_state.len() - 3..], &[0x02, 0xAA, 0xBB]);
        assert_eq!(&with_state[3..5], &[0x00, 0x1D]);
    }

    #[test]
    fn single_response_yields_the_rfcomm_channel() {
        let pdu = response(7, &lists(Some(5)), &[]);
        let r = parse_response(7, &pdu).unwrap();
        assert!(r.continuation.is_empty());
        assert_eq!(rfcomm_channel(r.data), Ok(Some(5)));
    }

    #[test]
    fn continuation_state_and_fragments_are_reassembled() {
        let whole = lists(Some(12));
        let (a, b) = whole.split_at(9);
        let first = response(3, a, &[0x01, 0x02]);
        let second = response(4, b, &[]);
        let r1 = parse_response(3, &first).unwrap();
        assert_eq!(r1.continuation, &[0x01, 0x02]);
        let r2 = parse_response(4, &second).unwrap();
        assert!(r2.continuation.is_empty());
        let mut joined = r1.data.to_vec();
        joined.extend_from_slice(r2.data);
        assert_eq!(rfcomm_channel(&joined), Ok(Some(12)));
        assert_eq!(parse_response(9, &first), Err(SdpError::WrongTransaction));
    }

    #[test]
    fn rfcomm_as_uuid128_inside_an_alternative_and_bad_channels() {
        let mut proto = vec![0x1C];
        proto.extend_from_slice(&UUID_RFCOMM_128);
        proto.extend_from_slice(&[0x08, 9]);
        let mut list = vec![0x08, 0x01]; // not a sequence: skipped
        list.extend(des(&proto));
        let mut record = vec![0x09, 0x00, 0x04];
        record.extend(des(&list));
        let mut dea = des(&record);
        dea[0] = 0x3D;
        assert_eq!(rfcomm_channel(&des(&dea)), Ok(Some(9)));
        for bad in [0u8, 31] {
            assert_eq!(rfcomm_channel(&lists(Some(bad))), Err(SdpError::Malformed));
        }
    }

    #[test]
    fn record_without_rfcomm_gives_none() {
        assert_eq!(rfcomm_channel(&lists(None)), Ok(None));
        assert_eq!(rfcomm_channel(&des(&[])), Ok(None));
    }

    #[test]
    fn truncated_and_malformed_input_is_an_error_not_a_panic() {
        let pdu = response(1, &lists(Some(5)), &[]);
        for n in 0..pdu.len() {
            assert!(parse_response(1, &pdu[..n]).is_err());
        }
        let data = lists(Some(5));
        for n in 0..data.len() {
            assert!(rfcomm_channel(&data[..n]).is_err());
        }
        // A sequence that claims more bytes than it has, a 4 byte length, junk.
        assert!(rfcomm_channel(&[0x35, 0x7F, 0x00]).is_err());
        assert!(rfcomm_channel(&[0x37, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
        assert!(rfcomm_channel(&[0x08, 0x01]).is_err());
        let err = [0x01, 0x00, 0x01, 0x00, 0x02, 0x00, 0x03];
        assert_eq!(parse_response(1, &err), Err(SdpError::ErrorResponse(3)));
        // Continuation state longer than the specification allows.
        let long = response(1, &[], &[0; 17]);
        assert_eq!(parse_response(1, &long), Err(SdpError::Malformed));
    }
}
