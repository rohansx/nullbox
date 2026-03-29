# nullctl -- CLI Tool

## Overview

nullctl is the command-line interface for managing a running NullBox instance. It communicates with nulld and cage over Unix domain sockets to query status, manage agent VMs, and initiate shutdown.

## Architecture

nullctl is a stateless client. It connects to a socket, sends a JSON request, reads the response, and exits. There is no persistent state or configuration file.

### Socket Communication

| Target | Socket Path | Purpose |
|--------|-------------|---------|
| nulld | `/run/nulld.sock` | Service status, system shutdown |
| cage | `/run/cage.sock` | Agent VM lifecycle (list, start, stop) |

### Request Flow

```
nullctl status
  -> connect(/run/nulld.sock)
  -> send: {"method": "status"}\n
  -> shutdown(Write)
  -> read response line
  -> pretty-print table
  -> exit
```

### Key Components

| File | Purpose |
|------|---------|
| `cmd/nullctl/src/main.rs` | CLI argument parsing, socket communication, output formatting |

## Configuration

nullctl has no configuration files. Socket paths are hardcoded:
- `/run/nulld.sock` for nulld commands
- `/run/cage.sock` for cage commands

## API / Protocol

### Commands

#### nullctl status

Query nulld for the status of all managed services.

```
$ nullctl status
SERVICE         STATE        PID      RESTARTS
egress          running      42       0
ctxgraph        running      43       0
cage            running      44       0
```

#### nullctl shutdown

Initiate a clean shutdown of all services and the system.

```
$ nullctl shutdown
nulld: shutdown initiated
```

#### nullctl cage list

List all running agent VMs.

```
$ nullctl cage list
AGENT                PID        VCPUS    RAM_MIB
researcher           100        2        512
```

When no agents are running:

```
$ nullctl cage list
no running agents
```

#### nullctl cage start \<agent\>

Start an agent microVM by name.

```
$ nullctl cage start researcher
cage: started 'researcher' (PID 100)
```

#### nullctl cage stop \<agent\>

Stop a running agent microVM.

```
$ nullctl cage stop researcher
cage: stopped 'researcher'
```

#### nullctl help

Print usage information.

```
$ nullctl help
nullctl -- NullBox CLI

usage:
  nullctl status              Show service status
  nullctl shutdown            Initiate clean shutdown
  nullctl cage list           List running agent VMs
  nullctl cage start <agent>  Start an agent microVM
  nullctl cage stop <agent>   Stop an agent microVM
```

### Error Handling

Connection failures produce clear error messages:

```
$ nullctl status
nullctl: error: cannot connect to /run/nulld.sock: No such file or directory
```

Service-level errors are forwarded from the daemon:

```
$ nullctl cage start nonexistent
cage: unknown agent: nonexistent
```

## Status

**Implemented:**
- Service status table (nulld)
- System shutdown command (nulld)
- Agent VM list with PID/vCPU/RAM columns (cage)
- Agent VM start and stop (cage)
- Pretty-printed output tables
- Error handling for socket connection failures

**Planned:**
- ctxgraph commands (query, read, write)
- egress commands (list rules, add/remove agent rules)
- Agent log tailing
- JSON output mode (--json flag)
- Shell completion
