#![no_main]
#![no_std]

use cortex_m_rt::entry;
use core::fmt::Write;
use panic_halt as _;
use nb::block;
use stm32f1xx_hal::{
    i2c::{BlockingI2c, DutyCycle, Mode},
    pac,
    prelude::*,
    serial,
    timer::Timer,
};

#[entry]
fn main() -> ! {
    
    // Get access to the core peripherals from the cortex-m crate
    let cp = cortex_m::Peripherals::take().unwrap();

    // Get access to the device specific peripherals from the peripheral access crate
    let dp = pac::Peripherals::take().unwrap();

    // Take ownership over the raw flash and rcc devices and convert them into the corresponding
    // HAL structs
    let mut flash = dp.FLASH.constrain();
    let rcc = dp.RCC.constrain();
    let mut afio = dp.AFIO.constrain();

    // Freeze the configuration of all the clocks in the system and store the frozen frequencies in
    // `clocks`
    let clocks = rcc.cfgr.sysclk(8.MHz()).freeze(&mut flash.acr);

    // Prepare the GPIOA peripheral, PA9 and PA10 for USART1
    let mut gpioa = dp.GPIOA.split();    
    let pin_tx = gpioa.pa9.into_alternate_push_pull(&mut gpioa.crh);
    let pin_rx = gpioa.pa10;    

    // Acquire GPIOB, PB6 and PB7 for I2C1
    let mut gpiob = dp.GPIOB.split();
    let scl = gpiob.pb6.into_alternate_open_drain(&mut gpiob.crl);
    let sda = gpiob.pb7.into_alternate_open_drain(&mut gpiob.crl);

    let mut timer = Timer::syst(cp.SYST, &clocks).counter_hz();

    struct Coeffs {
        ac5: i16,
        ac6: i16,
        mc: i16,
        md: i16,
    }

    let mut calib_coeffs = Coeffs {
        ac5: 0,
        ac6: 0,
        mc: 0,
        md: 0,
    };

    const BMP180_ADDR: u8 = 0x77;
    const REG_ID_ADDR: u8 = 0xD0;
    const AC5_MSB_ADDR: u8 = 0xB2;
    const AC6_MSB_ADDR: u8 = 0xB4;
    const MC_MSB_ADDR: u8 = 0xBC;
    const MD_MSB_ADDR: u8 = 0xBE;
    const CTRL_MEAS_ADDR: u8 = 0xF4;
    const MEAS_OUT_LSB_ADDR: u8 = 0xF7;
    const MEAS_OUT_MSB_ADDR: u8 = 0xF6;

    let mut rx_buffer: [u8; 2] = [0; 2];
    let mut rx_word: i16;

    // Set up the usart device. Take ownership over the USART register and tx/rx pins. The rest of
    // the registers are used to enable and configure the device.
    let mut serial = serial::Serial::new(
        dp.USART1,
        (pin_tx, pin_rx),
        &mut afio.mapr,
        serial::Config::default()
            .baudrate(9600.bps())
            .wordlength_8bits()
            .parity_none(),
        &clocks,
    );

    // Config I2C1
    let mut i2c = BlockingI2c::i2c1(
        dp.I2C1,
        (scl, sda),
        &mut afio.mapr,
        Mode::Fast {
            frequency: 400.kHz(),
            duty_cycle: DutyCycle::Ratio16to9,
        },
        clocks,
        1000,
        10,
        1000,
        1000,
    );

    timer.start(1.Hz()).unwrap();

    // STARTING!
    writeln!(serial, "Starting I2C Scan").unwrap();

    // I2C addresses are tipically 7 bits long, 0..127
    for address in 0..=127 {
        if i2c.write(address, &[1]).is_ok() {
            writeln!(serial, "Found device at address {:#04X?}", address).unwrap();
        }
    }

    // Read Device ID as Sanity Check
    i2c.write_read(BMP180_ADDR, &[REG_ID_ADDR], &mut rx_buffer).unwrap();

    if rx_buffer[0] == 0x55 {
        writeln!(serial, "Device ID is {}\r", rx_buffer[0]).unwrap();
    } else {
        writeln!(serial, "Device ID Cannot be Detected\r").unwrap();
    }

    // Read Calibration Coefficients
    // Read AC5
    i2c.write_read(BMP180_ADDR, &[AC5_MSB_ADDR], &mut rx_buffer)
        .unwrap();
    rx_word = ((rx_buffer[0] as i16) << 8) | rx_buffer[1] as i16;
    writeln!(serial, "AC5 = {} \r", rx_word).unwrap();
    calib_coeffs.ac5 = rx_word;

    // Read AC6
    i2c.write_read(BMP180_ADDR, &[AC6_MSB_ADDR], &mut rx_buffer)
        .unwrap();
    rx_word = ((rx_buffer[0] as i16) << 8) | rx_buffer[1] as i16;
    writeln!(serial, "AC6 = {} \r", rx_word).unwrap();
    calib_coeffs.ac6 = rx_word;

    // Read MC
    i2c.write_read(BMP180_ADDR, &[MC_MSB_ADDR], &mut rx_buffer)
        .unwrap();
    rx_word = ((rx_buffer[0] as i16) << 8) | rx_buffer[1] as i16;
    writeln!(serial, "MC = {} \r", rx_word).unwrap();
    calib_coeffs.mc = rx_word;

    // Read MD
    i2c.write_read(BMP180_ADDR, &[MD_MSB_ADDR], &mut rx_buffer)
        .unwrap();
    rx_word = ((rx_buffer[0] as i16) << 8) | rx_buffer[1] as i16;
    writeln!(serial, "MD = {} \r", rx_word).unwrap();
    calib_coeffs.md = rx_word;

    // Application Loop
    loop {
        // Kick off Temperature Measurement by writing 0x2E in register 0xF4
        i2c.write(BMP180_ADDR, &[CTRL_MEAS_ADDR, 0x2E]).unwrap();
        block!(timer.wait()).unwrap();

        // Collect Temperature Measurment

        // Read Measurement MSB
        i2c.write(BMP180_ADDR, &[MEAS_OUT_MSB_ADDR]).unwrap();
        i2c.read(BMP180_ADDR, &mut rx_buffer).unwrap();
        rx_word = (rx_buffer[0] as i16) << 8;

        // Read Measurement LSB
        i2c.write(BMP180_ADDR, &[MEAS_OUT_LSB_ADDR]).unwrap();
        i2c.read(BMP180_ADDR, &mut rx_buffer).unwrap();
        rx_word |= rx_buffer[0] as i16;

        // Calculate Temperature According to Datasheet Formulas
        let x1 = (rx_word as i32 - calib_coeffs.ac6 as i32) * (calib_coeffs.ac5 as i32) >> 15;
        let x2 = ((calib_coeffs.mc as i32) << 11) / (x1 + calib_coeffs.md as i32);
        let b5 = x1 + x2;
        let t = ((b5 + 8) >> 4) / 10;

        // Print Temperature Value
        writeln!(serial, "Temperature = {:} \r", t).unwrap();
    }
}
