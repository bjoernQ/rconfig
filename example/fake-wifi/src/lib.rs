const BLE_BUFFER: Option<&str> = option_env!("CONFIG_options_buffer");

pub fn awesome(){
    #[cfg(options_ble)]
    println!("BLE ENABLED");

    println!("BLE_BUFFER {:?}", BLE_BUFFER);
}
