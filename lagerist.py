#!/usr/bin/env python3

import os
import trio

from prometheus_client import generate_latest, Histogram

# Histogram buckets. The values given are milliseconds
# converted to seconds because I can't wrap my head
# around thinking about IOPS in seconds.
HISTOGRAM_BUCKETS = [bucket / 1000. for bucket in [
    0.01,  0.025,  0.05,  0.075,
    0.1,   0.25,   0.5,   0.75,
    1,     2.5,    5.0,   7.5,
   10,    25,     50,    75,
  100
]]


async def ktrace():
    histograms = (
        Histogram('diskio_queue_time_seconds', 'Time spent in the queue',                       labelnames=("device", "optype"), buckets=HISTOGRAM_BUCKETS),
        Histogram('diskio_disk_time_seconds',  'Time spent in the disk (aka service time)',     labelnames=("device", "optype"), buckets=HISTOGRAM_BUCKETS),
        Histogram('diskio_total_time_seconds', 'Total time taken by IO requests (aka latency)', labelnames=("device", "optype"), buckets=HISTOGRAM_BUCKETS),
    )
    devnames   = {}
    insertion  = {}
    issuance   = {}

    def get_dev_path(dev):
        # Dev is a "major,minor" string
        full_path = devnames.get(dev)
        if full_path is None:
            major, minor = [int(x) for x in dev.split(",")]
            with open("/proc/partitions") as fd:
                for line in fd:
                    line = line.strip()
                    if not line or line.startswith("major"):
                        continue
                    fields = line.split()
                    if int(fields[0]) == major and int(fields[1]) == minor:
                        name = fields[3]
                        break
                else:
                    # You know I've been through a desert on a
                    raise ValueError("device with no name: %d,%d" % (major, minor))

            if name.startswith("dm-"):
                # Search for a better name, this probably is an LV
                for link in os.listdir("/dev/mapper"):
                    link_target = os.path.realpath(os.path.join("/dev/mapper", link))
                    if link_target == "/dev/%s" % name:
                        if "-" in link:
                            vg, lv = link.split("-", 1)
                            lv = lv.replace("--", "-")
                            if os.path.exists(os.path.join("/dev", vg, lv)):
                                full_path = os.path.join("/dev", vg, lv)
                        full_path = os.path.join("/dev/mapper", link)
                        break
            else:
                full_path = "/dev/%s" % name

            devnames[dev] = full_path
        return full_path

    async with await trio.open_file("/sys/kernel/debug/tracing/instances/lagerist/trace_pipe", "rb") as f:
        async for line in f:
            parts = line.decode("utf-8").split()

            # The definition seems to be from here:
            # https://github.com/torvalds/linux/blob/master/include/trace/events/block.h#L175
            # Looks like the fields in this TP_printk are parts[5:]; parts[0:4] seem to be constant.

            # Unfortunately, part[0] can contain whitespace. m(
            # Since we don't really need part[0], let's pop(0) until parts[1] is the one with the numbers in brackets.

            while not parts[1].startswith("["):
                parts.pop(0)

            try:
                time = float(parts[3].rstrip(":"))
            except ValueError:
                print("Something's off about this line:", line)
                continue

            op   = parts[4].rstrip(":")
            dev  = parts[5]
            rwbs = parts[6]

            if dev == "0,0":
                # wtf
                continue

            if "R" in rwbs:
                optype = "read"
            elif "W" in rwbs:
                optype = "write"
            else:
                continue

            dev_path = get_dev_path(dev)

            if op == "block_rq_insert":
                # insert and issue ops have a request size field
                reqsz = parts[7]
                sector = parts[9]
                nr_sectors = parts[11]
                insertion["%s,%s,%s" % (dev, sector, nr_sectors)] = time

            if op == "block_rq_issue":
                # insert and issue ops have a request size field
                reqsz = parts[7]
                sector = parts[9]
                nr_sectors = parts[11]
                issuance["%s,%s,%s" % (dev, sector, nr_sectors)] = time

            elif op == "block_rq_complete":
                # complete ops do not have the size field
                reqsz = None
                sector = parts[8]
                nr_sectors = parts[10]

                key = "%s,%s,%s" % (dev, sector, nr_sectors)

                try:
                    queue_time = issuance[key] - insertion[key]
                    disk_time  = time - issuance[key]
                except KeyError:
                    # We probably haven't observed the insertion/issuance event because we weren't around back then
                    continue

                del insertion[key]
                del issuance[key]

                total_time = queue_time + disk_time

                histograms[0].labels(device=dev_path, optype=optype).observe(queue_time)
                histograms[1].labels(device=dev_path, optype=optype).observe(disk_time)
                histograms[2].labels(device=dev_path, optype=optype).observe(total_time)


async def http_handler(server_stream):
    try:
        async for data in server_stream:
            if data.startswith(b"GET /"):
                dump = generate_latest()
                await server_stream.send_all(b"".join([
                    b"HTTP/1.1 200 OK\n",
                    b"Content-Type: text/plain\n",
                    b"Content-Length: %d\n\n" % len(dump),
                    dump
                ]))
    except Exception as exc:
        print("http_handler: crashed: {!r}".format(exc))

async def httpd():
    await trio.serve_tcp(http_handler, 9789)

async def async_main():
    async with trio.open_nursery() as nursery:
        nursery.start_soon(ktrace)
        nursery.start_soon(httpd)

def main():
    if not os.path.exists("/sys/kernel/debug/tracing/instances"):
        raise SystemError("kernel debugging is disabled, please mount debugfs")

    # Initialize disk tracing. That goes a little something like this:
    #
    # INST="/sys/kernel/debug/tracing/instances/lagerist"
    # mkdir -p "$INST"
    # echo 1 > "$INST/events/block/block_rq_issue/enable"
    # echo 1 > "$INST/events/block/block_rq_insert/enable"
    # echo 1 > "$INST/events/block/block_rq_complete/enable"
    # echo 1 > "$INST/tracing_on"

    inst_dir = "/sys/kernel/debug/tracing/instances/lagerist"

    if not os.path.exists(inst_dir):
        os.mkdir(inst_dir)

    for event in ("block_rq_insert", "block_rq_issue", "block_rq_complete"):
        with open(os.path.join(inst_dir, "events/block", event, "enable"), "wb") as fd:
            fd.write(b"1")

    with open(os.path.join(inst_dir, "tracing_on"), "wb") as fd:
        fd.write(b"1")

    # Run trio to do the actual work
    try:
        trio.run(async_main)
    except KeyboardInterrupt:
        pass
    finally:
        # Tear down ktrace
        #
        # echo 0 > "$INST/tracing_on"
        # rmdir "$INST"

        with open(os.path.join(inst_dir, "tracing_on"), "wb") as fd:
            fd.write(b"0")

        os.rmdir(inst_dir)


if __name__ == '__main__':
    main()
