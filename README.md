# qdl - Sahara / Firehose tools, in Rust!

Qualcomm SoCs feature the Emergency Download Mode (EDL, widely known as '9008 mode'), a bootrom-initiated device flashing stack.

`qdl` provides a Rust implementation for the Sahara and Firehose protocols that are used to communicate with a device in that mode.

## Contents
```
cli/ - A CLI tool used to communicate with devices in EDL mode
qdl/ - Sahara / Firehose library, with USB convenience wrappers
qramdump/ - Tool to receive memory dumps from a crashed device
```

## Building
Run `cargo build [--release]` to build all executables within this repo.

Use `cargo run [--release] --bin <executable_name> [-- extra_args]` to quickly build one of the programs from source and run it.

## Running qdl
```
Usage: qdl-rs [OPTIONS] --loader-path <FILE> --storage-type <emmc/ufs/nvme/nand> <COMMAND>

Commands:
  dump               Dump the entire storage
  dump-part          Dump a single partition
  flasher            Invoke the flasher
  erase              Erase a partition
  nop                Ask the device to do nothing, hopefully successfully
  overwrite-storage  Overwrite the storage physical partition contents with a raw image Similar to Flasher, but this one only takes a partition dump as input and performs no real validation on the input data
  peek               Peek at memory
  print-gpt          Print the GPT table
  set-bootable-part  Mark physical storage partition as bootable
  write              Write a partition
  help               Print this message or the help of the given subcommand(s)

Options:
      --backend <usb/serial>
  -d, --dev-path <DEV_PATH>                E.g. COM4 on Windows
  -l, --loader-path <FILE>
      --hash-packets                       Validate every packet. Slow.
      --phys-part-idx <PHYS_PART_IDX>      [default: 0]
      --print-firehose-log
      --read-back-verify                   Every <program> operation is read back. VERY SLOW!
      --reset-mode <edl/off/system>        [default: edl]
      --serial-no <SERIAL_NO>
  -A, --skip-hello-wait                    Work around missing HELLO packet
  -s, --storage-type <emmc/ufs/nvme/nand>
      --sector-size <SECTOR_SIZE>
      --skip-write
      --skip-storage-init                  Required for unprovisioned storage media.
      --verbose-sahara
      --verbose-firehose
  -h, --help                               Print help
  -V, --version                            Print version
```

## Running qramdump
```
Usage: qramdump [OPTIONS] [REGIONS_TO_DUMP]...

Arguments:
  [REGIONS_TO_DUMP]...

Options:
      --backend <usb/serial>
  -d, --dev-path <DEV_PATH>    E.g. COM4 on Windows
      --serial-no <SERIAL_NO>
      --verbose-sahara
  -h, --help                   Print help
  -V, --version                Print version
```

### Windows
You'll need to acquire an appropriate driver that exposes the device as a USB serial port, or use [WinUSB](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/winusb-installation).

Serial is used as the default backend on this platform.

## Flashing a full META image
### Example with UFS as primary storage, reboots to OS after flashing ends
```
qdl-rs -l prog_firehose_ddr.elf -s ufs [--serial-no abcd1234] --reset-mode system flasher -p rawprogram*.xml -x patch*.xml
```

## Documentation

Run `cargo doc --open` to generate and open the latest rustdoc. Learn more [here](https://doc.rust-lang.org/cargo/commands/cargo-doc.html).

## License

See the [`LICENSE` file](/LICENSE).

## Credits

* [linux-msm/qdl](https://github.com/linux-msm/qdl) for the open C implementation
