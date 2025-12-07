use prost_build;

fn main() {
    prost_build::compile_protos(&["src/etiquette_0.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_1.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_2.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_3.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_4.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_5.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_6.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_7.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_8.proto"], &["src/"]).unwrap();
}
