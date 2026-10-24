fn main() {
    println!("Sample: {} bytes", std::mem::size_of::<fancore::protocol::Sample>());
    println!("SensorReading: {} bytes", std::mem::size_of::<fancore::protocol::SensorReading>());
    println!("4096 samples: {} KB", 4096 * std::mem::size_of::<fancore::protocol::Sample>() / 1024);
    println!("3600 samples: {} KB", 3600 * std::mem::size_of::<fancore::protocol::Sample>() / 1024);
}
