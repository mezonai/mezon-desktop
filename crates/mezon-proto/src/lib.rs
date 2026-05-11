// mezon-proto: prost-generated Mezon API and realtime protobuf types.

pub mod api {
    include!(concat!(env!("OUT_DIR"), "/mezon.api.rs"));
}

pub mod realtime {
    include!(concat!(env!("OUT_DIR"), "/mezon.realtime.rs"));
}
