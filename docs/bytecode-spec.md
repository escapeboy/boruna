# Boruna Bytecode Specification (.axbc)

## Binary Format

```
Offset  Size  Description
0       4     Magic bytes: 0x4C 0x4C 0x4D 0x42 ("LLMB")
4       2     Version (little-endian u16, currently 1)
6       4     Payload length (little-endian u32)
10      N     JSON-encoded Module payload
```

## Module Structure (JSON)

```json
{
  "name": "module_name",
  "version": 1,
  "constants": [...],
  "globals": [...],
  "types": [...],
  "functions": [...],
  "entry": 0
}
```

## Instruction Set

### Stack Operations
| Opcode | Byte | Description |
|--------|------|-------------|
| PushConst(idx) | 0x01 | Push constant from pool |
| Pop | 0x60 | Discard top of stack |
| Dup | 0x61 | Duplicate top of stack |

### Local/Global Variables
| Opcode | Byte | Description |
|--------|------|-------------|
| LoadLocal(idx) | 0x02 | Push local variable |
| StoreLocal(idx) | 0x03 | Pop into local variable |
| LoadGlobal(idx) | 0x04 | Push global variable |
| StoreGlobal(idx) | 0x05 | Pop into global variable |

### Control Flow
| Opcode | Byte | Description |
|--------|------|-------------|
| Call(fn, arity) | 0x06 | Call function |
| Ret | 0x07 | Return from function |
| Jmp(offset) | 0x08 | Unconditional jump |
| JmpIf(offset) | 0x09 | Jump if truthy |
| JmpIfNot(offset) | 0x0A | Jump if falsy |
| Match(table) | 0x0B | Pattern match |

### Data Construction
| Opcode | Byte | Description |
|--------|------|-------------|
| MakeRecord(type, n) | 0x0C | Create record from N stack values |
| MakeEnum(type, var) | 0x0D | Create enum variant |
| GetField(idx) | 0x0E | Access record field |

### Actor Model
| Opcode | Byte | Description |
|--------|------|-------------|
| SpawnActor(fn) | 0x0F | Spawn new actor |
| SendMsg | 0x10 | Send message to actor |
| ReceiveMsg | 0x11 | Block until message arrives |

### Capabilities
| Opcode | Byte | Description |
|--------|------|-------------|
| Assert(err) | 0x12 | Assert truthy or abort |
| CapCall(cap, arity) | 0x13 | Invoke capability |

### Arithmetic (0x20-0x25)
Add, Sub, Mul, Div, Mod, Neg

### Comparison (0x30-0x35)
Eq, Neq, Lt, Lte, Gt, Gte

### Logical (0x40-0x42)
Not, And, Or

### String (0x50)
Concat

### UI (0x70)
EmitUi — Emit declarative UI tree

### List Operations (0x80-0x83)
| Opcode | Byte | Description |
|--------|------|-------------|
| MakeList(n) | 0x80 | Pop N values, create List |
| ListLen | 0x81 | Pop list, push length as Int |
| ListGet | 0x82 | Pop list and index, push element |
| ListPush | 0x83 | Pop list and value, push new list with value appended |

### String Builtins (0x84-0x86)
| Opcode | Byte | Description |
|--------|------|-------------|
| ParseInt | 0x84 | Pop string, push parsed Int (0 on failure) |
| TryParseInt | 0x87 | Pop string, push Result: Ok(Int) or Err(String) |
| StrContains | 0x85 | Pop haystack and needle, push Bool |
| StrStartsWith | 0x86 | Pop string and prefix, push Bool |

### Control (0xFE-0xFF)
Nop, Halt

## Capability IDs

| ID | Name | Description |
|----|------|-------------|
| 0 | net.fetch | Network requests |
| 1 | fs.read | File system read |
| 2 | fs.write | File system write |
| 3 | db.query | Database queries |
| 4 | ui.render | UI rendering |
| 5 | time.now | Current time |
| 6 | random | Random number generation |
