# ckb-vm-signal-profiler

Signal based profiler for [ckb-vm](https://github.com/nervosnetwork/ckb-vm). Like [gperftools](https://github.com/gperftools/gperftools), it uses a `SIGPROF` signal handler to suspend running CKB-VM programs so as to gather profiling data. One advantage of this solution, is that it requires no code injections into CKB-VM. However also due to this design choice, this profiler runs on Linux only for the moment.

See [./examples/simple.rs] for a simple usage of this library.

This library inherits a lot of the signal handler related code from [pprof-rs](https://github.com/tikv/pprof-rs) library.
