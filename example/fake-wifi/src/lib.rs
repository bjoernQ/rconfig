rconfig::include_config!();

pub fn awesome(){
    #[cfg(options_ble)]
    println!("BLE ENABLED");

    #[cfg(has_options_buffer)]
    println!("BLE_BUFFER {:?}", OPTIONS_BUFFER);
}
