//! The request frames we generate must be byte-identical to the host frames
//! the old vendor app sent during its initial full read
//! (py-configurator capture `read_01_initial_load.json`, FW 1.17).
//! Only the 8-byte headers are kept: every captured request had an all-zero payload.

use type84_proto::transfer::request_frames;
use type84_proto::ReadOp;

const CAPTURED: &[(ReadOp, &[&str])] = &[
    (ReadOp::DeviceInfo, &["aa10300000000100"]),
    (ReadOp::GameMode, &["aa11380000000100"]),
    (
        ReadOp::Keymap,
        &[
            "aa12380000000000",
            "aa12383800000000",
            "aa12387000000000",
            "aa1238a800000000",
            "aa1238e000000000",
            "aa12381801000000",
            "aa12385001000000",
            "aa12388801000000",
            "aa1238c001000000",
            "aa1208f801000100",
        ],
    ),
    (
        ReadOp::FnKeymap,
        &[
            "aa16380000000000",
            "aa16383800000000",
            "aa16387000000000",
            "aa1638a800000000",
            "aa1638e000000000",
            "aa16381801000000",
            "aa16385001000000",
            "aa16388801000000",
            "aa1638c001000000",
            "aa1608f801000100",
        ],
    ),
    (
        ReadOp::DefaultFnKeymap,
        &[
            "aa1c380000000000",
            "aa1c383800000000",
            "aa1c387000000000",
            "aa1c38a800000000",
            "aa1c38e000000000",
            "aa1c381801000000",
            "aa1c385001000000",
            "aa1c388801000000",
            "aa1c38c001000000",
            "aa1c08f801000100",
        ],
    ),
    (ReadOp::LedEffect, &["aa13100000000100"]),
    (
        ReadOp::CustomLed,
        &[
            "aa14380000000000",
            "aa14383800000000",
            "aa14387000000000",
            "aa1438a800000000",
            "aa1438e000000000",
            "aa14381801000000",
            "aa14385001000000",
            "aa14388801000000",
            "aa1438c001000000",
            "aa1408f801000100",
        ],
    ),
    (
        ReadOp::Macro,
        &[
            "aa15380000000000",
            "aa15383800000000",
            "aa15387000000000",
            "aa1538a800000000",
            "aa1538e000000000",
            "aa15381801000000",
            "aa15385001000000",
            "aa15088801000100",
        ],
    ),
    (
        ReadOp::AxisRt,
        &[
            "aa17380000000000",
            "aa17383800000000",
            "aa17387000000000",
            "aa1738a800000000",
            "aa1738e000000000",
            "aa17381801000000",
            "aa17385001000000",
            "aa17388801000000",
            "aa1738c001000000",
            "aa1738f801000000",
            "aa17383002000000",
            "aa17386802000000",
            "aa1738a002000000",
            "aa1738d802000000",
            "aa17381003000000",
            "aa17384803000000",
            "aa17388003000000",
            "aa1738b803000000",
            "aa1710f003000100",
        ],
    ),
    (
        ReadOp::Dks,
        &[
            "aa18380000000000",
            "aa18383800000000",
            "aa18387000000000",
            "aa1838a800000000",
            "aa1838e000000000",
            "aa18381801000000",
            "aa18385001000000",
            "aa18388801000000",
            "aa1838c001000000",
            "aa1838f801000000",
            "aa18383002000000",
            "aa18386802000000",
            "aa1838a002000000",
            "aa1838d802000000",
            "aa18381003000000",
            "aa18384803000000",
            "aa18388003000000",
            "aa1838b803000000",
            "aa1810f003000100",
        ],
    ),
];

#[test]
fn generated_requests_match_capture() {
    for (op, headers) in CAPTURED {
        let frames = request_frames(*op, 0, op.block_size()).unwrap();
        assert_eq!(frames.len(), headers.len(), "{op:?} packet count");
        for (f, h) in frames.iter().zip(headers.iter()) {
            assert_eq!(hex::encode(&f[..8]), *h, "{op:?}");
            assert!(f[8..].iter().all(|&b| b == 0));
        }
    }
}
