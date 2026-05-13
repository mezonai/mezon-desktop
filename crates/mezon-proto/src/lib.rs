// mezon-proto: prost-generated Mezon API and realtime protobuf types.

pub mod api {
    #![allow(clippy::empty_docs, clippy::large_enum_variant)]

    include!(concat!(env!("OUT_DIR"), "/mezon.api.rs"));
}

pub mod realtime {
    #![allow(clippy::empty_docs, clippy::large_enum_variant)]

    include!(concat!(env!("OUT_DIR"), "/mezon.realtime.rs"));
}
