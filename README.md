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
You're expected to have a recent installation of Rust. You can acquire one with [rustup](https://rustup.rs).
</br>If you already have an older installation, try `rustup update`.

Run `cargo build [--release]` to build all executables within this repo. The binaries will appear in `target/debug/` or `target/release`, respectively.

Use `cargo run [--release] --bin <qdl-rs/qramdump> [-- args]` to quickly build one of the programs from source and run it.

## Running the programs
<details>

<summary>qdl-rs</summary>

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

      --bypass-storage
          Accept storage r/w operations, but make them never actually execute (useful for testing USB throughput)
  -d, --dev-path <DEV_PATH>
          E.g. COM4 on Windows
  -l, --loader-path <FILE>

      --hash-packets
          Validate every packet. Slow.
  -L, --phys-part-idx <PHYS_PART_IDX>
          e.g. LUN index for UFS [default: 0]
      --print-firehose-log

      --read-back-verify
          Every <program> operation is read back. VERY SLOW!
      --reset-mode <edl/off/system>
          [default: edl]
      --serial-no <SERIAL_NO>

  -A, --skip-hello-wait
          Work around missing HELLO packet
  -s, --storage-type <emmc/ufs/nvme/nand>

      --sector-size <SECTOR_SIZE>

      --skip-storage-init
          Required for unprovisioned storage media.
      --verbose-sahara

      --verbose-firehose

  -h, --help
          Print help
  -V, --version
          Print version
```

</details>

<details>

<summary>qramdump</summary>

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

</details>

### Windows
You'll need to acquire an appropriate driver that exposes the device as a USB serial port, or use [WinUSB](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/winusb-installation).

Serial is used as the default backend on this platform.

### Loader filename on newer platforms
Some newer platforms (e.g. SM8750) require that a file called `xbl_s_devprg_ns.melf` is used instead of `prog_firehose_ddr.elf`. This change may be opaque to you if the file has been renamed as part of the binary delivery process.

### LUN handling
Due to how the protocol is constructed, particularly when interfacing with UFS, you ***must*** specify the LUN (physical storage partition) index on which you want to operate. This does not concern the `flasher` command (rawproramN.xml files include that information) and operations that aren't storage-related (e.g. `peek` or `nop`).

## Common usage examples

<details>
<summary>Flash a full META image</summary>
  
### Example with UFS as primary storage, reboots to OS after flashing ends
```
qdl-rs -l prog_firehose_ddr.elf -s ufs --reset-mode system flasher -p rawprogram*.xml -x patch*.xml

# NOTE: qdl-rs will flash anything you pass as a parameter. Some METAs ship a number of rawprogram0_foo.xml
# files which may be undesirable (e.g. _WIPE_GPT). You can filter those out with e.g.:
find /path/to/build/ -regex '.*/rawprogram[0-9]+\.xml$'
```

</details>

<details>
<summary>Dump the entire physical storage partition (e.g. LUN)</summary>
  
```
qdl-rs -l prog_firehose_ddr.elf -s ufs --phys-part-idx 2 dump -o lun2/
```

</details>

<details>

<summary>Fetch a single partition from LUN2</summary>

```
qdl-rs -l prog_firehose_ddr.elf -s ufs --phys-part-idx 2 dump-part EFI
```

</details>

<details>

<summary>Overwrite a single partition on LUN0</summary>

```
qdl-rs -l prog_firehose_ddr.elf -s ufs --phys-part-idx 0 write boot boot.img
```

</details>

<details>

<summary>Print out the partition table on LUN4</summary>

```
qdl-rs -l prog_firehose_ddr.elf -s ufs --phys-part-idx 4 print-gpt
```

</details>

<details>

<summary>Overwrite the entirety of LUN7 (VERY dangerous, may remove device-unique data)</summary>

```
qdl-rs -l prog_firehose_ddr.elf -s ufs --phys-part-idx 7 overwrite-storage lun7_dump.img
```

</details>

<details>
  
<summary>Erase a partition on eMMC (VERY dangerous, may remove device-unique data)</summary>

```
qdl-rs -l prog_firehose_ddr.elf -s emmc erase boot
```

</details>

<details>

<summary>Set LUN2 as bootable (i.e. containing xbl)</summary>

```
qdl-rs -l prog_firehose_ddr.elf -s ufs set-bootable-part 2
```

</details>

## Documentation

Run `cargo doc --open` to generate and open the latest rustdoc. Learn more [here](https://doc.rust-lang.org/cargo/commands/cargo-doc.html).

## Contributing

See [`CONTRIBUTING.md`](/CONTRIBUTING.md).
Your code is expected to pass `cargo fmt` and `cargo clippy` checks - the CI will be on the lookout for that.

## License

See the [`LICENSE` file](/LICENSE).

## Credits

* [linux-msm/qdl](https://github.com/linux-msm/qdl) for the open C implementation
