rconfig::include_config!();

pub fn awesome(){
    println!("Heapsize={}", HEAP_SIZE);

    #[cfg(psram_enable)]
    {
        println!("config psram_enable present");

        println!("psram size = {:?}", PSRAM_SIZE);
        println!("psram type = {:?}", PSRAM_TYPE_TYPE);
    }
}
