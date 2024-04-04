const HEAP_SIZE: Option<&str> = option_env!("CONFIG_heap_size");

pub fn awesome(){
    println!("Heapsize={:?}", HEAP_SIZE);

    #[cfg(psram_enable)]
    {
        println!("config psram_enable present");

        println!("psram size = {:?}", option_env!("CONFIG_psram_size"));
        println!("psram type = {:?}", option_env!("CONFIG_psram_type_type"));
    }
}
