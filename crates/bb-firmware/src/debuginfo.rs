//! ELF/DWARF debug information for source-level debugging of AVR firmware.
//!
//! Parsed once into owned tables (so it can be shared across threads behind an `Arc`):
//! line table (address ↔ file:line), functions with their parameters/locals (lexical-block
//! scoped, with location expressions or location lists), globals, a small type model (base,
//! pointer, array, struct, enum, typedef, cv-qualifiers), the ELF symbol table, and the raw
//! `.debug_frame` for CFI unwinding.
//!
//! AVR conventions: code addresses are byte addresses; data addresses in DWARF/ELF carry the
//! `0x800000` offset; DWARF registers 0–31 are r0–r31, 32 is SP, 36 is the return address
//! column; `bregN` on 26/28/30 means the 16-bit X/Y/Z pair.

use crate::FirmwareError;
use bb_avr::Avr;
use gimli::{EndianSlice, LittleEndian, Reader as _, UnwindSection as _};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

type R<'a> = EndianSlice<'a, LittleEndian>;

/// Strips the AVR data-space offset (`0x800000`) from an ELF/DWARF address.
pub fn data_addr(a: u64) -> u32 {
    if (0x80_0000..0x81_0000).contains(&a) {
        (a - 0x80_0000) as u32
    } else {
        a as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LineRow {
    pub addr: u32,
    pub file: u32,
    pub line: u32,
    pub column: u32,
    pub is_stmt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sequence {
    start: u32,
    end: u32,
    rows: Vec<LineRow>,
}

/// A DWARF location expression with the encoding needed to evaluate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub bytes: Vec<u8>,
    pub encoding: gimli::Encoding,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VarLocation {
    Expr(Expr),
    /// (begin, end, expression) entries.
    List(Vec<(u32, u32, Expr)>),
    Const(i64),
    OptimizedOut,
}

pub type TypeId = usize;

/// The address of a variable located by a single `DW_OP_addr`.
pub fn static_address(loc: &VarLocation) -> Option<u64> {
    match loc {
        VarLocation::Expr(e) if e.bytes.len() == 5 && e.bytes[0] == 0x03 => {
            Some(u32::from_le_bytes([e.bytes[1], e.bytes[2], e.bytes[3], e.bytes[4]]) as u64)
        }
        _ => None,
    }
}

/// Reads a byte at an ELF data-space address: SRAM/I/O (0x80xxxx) or EEPROM (0x81xxxx).
fn peek_any(avr: &Avr, a: u64) -> u8 {
    if (0x81_0000..0x82_0000).contains(&a) {
        avr.eeprom()
            .get((a - 0x81_0000) as usize)
            .copied()
            .unwrap_or(0xFF)
    } else {
        avr.peek(data_addr(a) as u16)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum TypeInfo {
    Base {
        name: String,
        size: u32,
        encoding: u8,
    },
    Pointer {
        target: Option<TypeId>,
        size: u32,
    },
    Array {
        elem: Option<TypeId>,
        count: Option<u32>,
    },
    Struct {
        name: String,
        size: u32,
        members: Vec<(String, u32, Option<TypeId>)>,
    },
    Enum {
        name: String,
        size: u32,
        values: Vec<(String, i64)>,
    },
    Typedef {
        name: String,
        target: Option<TypeId>,
    },
    Qualified {
        qual: &'static str,
        target: Option<TypeId>,
    },
    Void,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub name: String,
    pub ty: Option<TypeId>,
    pub location: VarLocation,
    pub is_param: bool,
    /// Address ranges in which the variable is in scope (empty = whole function).
    pub scope: Vec<(u32, u32)>,
    pub decl_line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub linkage_name: Option<String>,
    pub low_pc: u32,
    pub high_pc: u32,
    pub decl_file: Option<PathBuf>,
    pub decl_line: u32,
    pub frame_base: Option<Expr>,
    pub vars: Vec<Variable>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Symbol {
    pub name: String,
    pub addr: u32,
    pub size: u32,
    pub is_function: bool,
}

/// A source position.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: u32,
    pub column: u32,
}

/// Debug information extracted from an ELF.
#[derive(Debug, Clone)]
pub struct DebugInfo {
    files: Vec<PathBuf>,
    sequences: Vec<Sequence>,
    pub functions: Vec<Function>,
    pub globals: Vec<Variable>,
    pub types: Vec<TypeInfo>,
    pub symbols: Vec<Symbol>,
    debug_frame: Vec<u8>,
    /// Bytes of the program counter on the stack (2, or 3 on the 2560).
    pub pc_bytes: u32,
}

// ------------------------------------------------------------------ values

/// A decoded variable value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Value {
    Int(i64),
    UInt(u64),
    Float(f64),
    Bool(bool),
    Char(u8),
    Pointer { addr: u32, string: Option<String> },
    Array(Vec<Value>),
    Struct(Vec<(String, Value)>),
    Enum { value: i64, name: Option<String> },
    OptimizedOut,
    Unavailable(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v}"),
            Value::UInt(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Char(c) => {
                if c.is_ascii_graphic() || *c == b' ' {
                    write!(f, "{} '{}'", c, *c as char)
                } else {
                    write!(f, "{c} '\\x{c:02x}'")
                }
            }
            Value::Pointer { addr, string } => match string {
                Some(s) => write!(f, "0x{addr:04x} \"{s}\""),
                None => write!(f, "0x{addr:04x}"),
            },
            Value::Array(v) => {
                write!(f, "{{")?;
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{x}")?;
                }
                write!(f, "}}")
            }
            Value::Struct(m) => {
                write!(f, "{{")?;
                for (i, (n, x)) in m.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{n} = {x}")?;
                }
                write!(f, "}}")
            }
            Value::Enum { value, name } => match name {
                Some(n) => write!(f, "{n}"),
                None => write!(f, "{value}"),
            },
            Value::OptimizedOut => write!(f, "<optimized out>"),
            Value::Unavailable(why) => write!(f, "<{why}>"),
        }
    }
}

/// A variable read from a running core.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VarValue {
    pub name: String,
    pub type_name: String,
    pub value: Value,
    /// Data address if the variable lives in memory.
    pub address: Option<u32>,
    /// Where it lives: "0x0127", "r24:r25", "const", "optimized out".
    pub location: String,
    pub is_param: bool,
}

/// Register state of one stack frame (for evaluating locations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRegs {
    pub pc: u32,
    pub regs: [u8; 32],
    /// Which registers are known (outer frames only know callee-saved ones).
    pub known: u32,
    pub sp: u32,
    pub cfa: Option<u32>,
}

impl FrameRegs {
    pub fn from_avr(avr: &Avr) -> FrameRegs {
        FrameRegs {
            pc: avr.pc(),
            regs: *avr.regs(),
            known: u32::MAX,
            sp: avr.sp() as u32,
            cfa: None,
        }
    }
    fn pair(&self, r: usize) -> u32 {
        self.regs[r] as u32 | (self.regs[r + 1] as u32) << 8
    }
}

// ------------------------------------------------------------------ parsing

struct TypeCtx<'a> {
    map: HashMap<usize, TypeId>,
    types: Vec<TypeInfo>,
    _p: std::marker::PhantomData<&'a ()>,
}

fn attr_str<'a>(
    dwarf: &gimli::Dwarf<R<'a>>,
    unit: &gimli::Unit<R<'a>>,
    e: &gimli::DebuggingInformationEntry<R<'a>>,
    at: gimli::DwAt,
) -> Option<String> {
    let v = e.attr_value(at)?;
    let s = dwarf.attr_string(unit, v).ok()?;
    Some(s.to_string_lossy().into_owned())
}

fn attr_u64<'a>(e: &gimli::DebuggingInformationEntry<R<'a>>, at: gimli::DwAt) -> Option<u64> {
    e.attr_value(at).and_then(|v| v.udata_value())
}

fn expr_from<'a>(e: gimli::Expression<R<'a>>, enc: gimli::Encoding) -> Expr {
    Expr {
        bytes: e.0.to_slice().map(|c| c.into_owned()).unwrap_or_default(),
        encoding: enc,
    }
}

/// Follows DW_AT_abstract_origin / DW_AT_specification within the unit.
fn origin<'a>(
    unit: &gimli::Unit<R<'a>>,
    e: &gimli::DebuggingInformationEntry<R<'a>>,
) -> Option<gimli::DebuggingInformationEntry<R<'a>>> {
    for at in [gimli::DW_AT_abstract_origin, gimli::DW_AT_specification] {
        if let Some(gimli::AttributeValue::UnitRef(off)) = e.attr_value(at) {
            return unit.entry(off).ok();
        }
    }
    None
}

fn name_of<'a>(
    dwarf: &gimli::Dwarf<R<'a>>,
    unit: &gimli::Unit<R<'a>>,
    e: &gimli::DebuggingInformationEntry<R<'a>>,
) -> Option<String> {
    attr_str(dwarf, unit, e, gimli::DW_AT_name).or_else(|| {
        let o = origin(unit, e)?;
        attr_str(dwarf, unit, &o, gimli::DW_AT_name).or_else(|| {
            let o2 = origin(unit, &o)?;
            attr_str(dwarf, unit, &o2, gimli::DW_AT_name)
        })
    })
}

impl DebugInfo {
    /// Parses debug information from an AVR ELF file's bytes.
    pub fn from_elf(elf: &[u8]) -> Result<DebugInfo, FirmwareError> {
        use object::{Object, ObjectSection, ObjectSymbol};
        let file = object::File::parse(elf).map_err(|e| FirmwareError::Elf(e.to_string()))?;
        let section = |name: &str| -> &[u8] {
            file.section_by_name(name)
                .and_then(|s| s.data().ok())
                .unwrap_or(&[])
        };
        let dwarf = gimli::Dwarf::load(|id| {
            Ok::<_, gimli::Error>(EndianSlice::new(section(id.name()), LittleEndian))
        })
        .map_err(|e: gimli::Error| FirmwareError::Dwarf(e.to_string()))?;
        let is_2560 = file
            .symbols()
            .any(|s| s.name() == Ok("__RAMPZ__") || s.name() == Ok("__EIND__"))
            || file
                .sections()
                .any(|s| s.name().is_ok_and(|n| n == ".text") && s.size() > 128 * 1024);
        let mut info = DebugInfo {
            files: Vec::new(),
            sequences: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
            types: Vec::new(),
            symbols: Vec::new(),
            debug_frame: section(".debug_frame").to_vec(),
            pc_bytes: if is_2560 { 3 } else { 2 },
        };
        // Symbols.
        for s in file.symbols() {
            let Ok(name) = s.name() else { continue };
            if name.is_empty() {
                continue;
            }
            let is_function = s.kind() == object::SymbolKind::Text;
            let is_data = s.kind() == object::SymbolKind::Data;
            if !is_function && !is_data {
                continue;
            }
            info.symbols.push(Symbol {
                name: name.to_string(),
                addr: data_addr(s.address()),
                size: s.size() as u32,
                is_function,
            });
        }
        info.symbols.sort_by_key(|s| s.addr);
        let mut file_ids: HashMap<PathBuf, u32> = HashMap::new();
        let mut tcx = TypeCtx {
            map: HashMap::new(),
            types: Vec::new(),
            _p: std::marker::PhantomData,
        };
        let mut units = dwarf.units();
        while let Some(header) = units
            .next()
            .map_err(|e| FirmwareError::Dwarf(e.to_string()))?
        {
            let unit = dwarf
                .unit(header)
                .map_err(|e| FirmwareError::Dwarf(e.to_string()))?;
            info.parse_lines(&dwarf, &unit, &mut file_ids)?;
            info.parse_dies(&dwarf, &unit, &mut tcx)?;
        }
        info.types = tcx.types;
        // avr-libc's assembler units describe SFRs (UDR0, TCNT1, …) as globals in I/O space;
        // they belong in the I/O register view, not the variable list.
        let io_end = if info.pc_bytes == 3 { 0x200 } else { 0x100 };
        // A DW_OP_addr of 0 marks a variable whose section the linker discarded (--gc-sections).
        info.globals.retain(|g| {
            !g.name.starts_with("__")
                && static_address(&g.location)
                    .is_none_or(|a| a != 0 && !(0x80_0000..0x80_0000 + io_end).contains(&a))
        });
        info.sequences.sort_by_key(|s| s.start);
        info.functions.sort_by_key(|f| f.low_pc);
        Ok(info)
    }

    fn parse_lines<'a>(
        &mut self,
        dwarf: &gimli::Dwarf<R<'a>>,
        unit: &gimli::Unit<R<'a>>,
        file_ids: &mut HashMap<PathBuf, u32>,
    ) -> Result<(), FirmwareError> {
        let Some(program) = unit.line_program.clone() else {
            return Ok(());
        };
        let comp_dir = unit
            .comp_dir
            .map(|d| PathBuf::from(d.to_string_lossy().as_ref()));
        let mut rows = program.rows();
        let mut seq: Vec<LineRow> = Vec::new();
        let mut cache: HashMap<u64, u32> = HashMap::new();
        while let Some((header, row)) = rows
            .next_row()
            .map_err(|e| FirmwareError::Dwarf(e.to_string()))?
        {
            let addr = row.address() as u32;
            if row.end_sequence() {
                if let Some(first) = seq.first() {
                    self.sequences.push(Sequence {
                        start: first.addr,
                        end: addr,
                        rows: std::mem::take(&mut seq),
                    });
                }
                continue;
            }
            let fidx = row.file_index();
            let file = match cache.get(&fidx) {
                Some(f) => *f,
                None => {
                    let mut p = PathBuf::new();
                    if let Some(fe) = row.file(header) {
                        if let Some(dir) = fe.directory(header)
                            && let Ok(d) = dwarf.attr_string(unit, dir)
                        {
                            p.push(d.to_string_lossy().as_ref());
                        }
                        if let Ok(n) = dwarf.attr_string(unit, fe.path_name()) {
                            p.push(n.to_string_lossy().as_ref());
                        }
                    }
                    if p.is_relative()
                        && let Some(cd) = &comp_dir
                    {
                        p = cd.join(p);
                    }
                    let id = *file_ids.entry(p.clone()).or_insert_with(|| {
                        self.files.push(p);
                        (self.files.len() - 1) as u32
                    });
                    cache.insert(fidx, id);
                    id
                }
            };
            let line = row.line().map(|l| l.get() as u32).unwrap_or(0);
            let column = match row.column() {
                gimli::ColumnType::LeftEdge => 0,
                gimli::ColumnType::Column(c) => c.get() as u32,
            };
            seq.push(LineRow {
                addr,
                file,
                line,
                column,
                is_stmt: row.is_stmt(),
            });
        }
        Ok(())
    }

    fn parse_dies<'a>(
        &mut self,
        dwarf: &gimli::Dwarf<R<'a>>,
        unit: &gimli::Unit<R<'a>>,
        tcx: &mut TypeCtx<'a>,
    ) -> Result<(), FirmwareError> {
        let enc = unit.encoding();
        let mut cursor = unit.entries();
        #[derive(Clone, PartialEq)]
        enum Scope {
            /// Compile unit / namespace: variables here are globals.
            Global,
            Function(usize, Vec<(u32, u32)>),
            /// Inlined bodies, declarations, types: variables are skipped.
            Skip,
        }
        let mut stack: Vec<(isize, Scope)> = Vec::new();
        let err = |e: gimli::Error| FirmwareError::Dwarf(e.to_string());
        while let Some(entry) = cursor.next_dfs().map_err(err)? {
            let depth = entry.depth();
            while stack.last().is_some_and(|s| s.0 >= depth) {
                stack.pop();
            }
            let parent = stack.last().map(|s| s.1.clone()).unwrap_or(Scope::Global);
            match entry.tag() {
                gimli::DW_TAG_compile_unit | gimli::DW_TAG_namespace => stack.push((depth, parent)),
                gimli::DW_TAG_subprogram => {
                    let mut ranges = Vec::new();
                    if let Ok(mut it) = dwarf.die_ranges(unit, entry) {
                        while let Ok(Some(r)) = it.next() {
                            if r.end > r.begin {
                                ranges.push((r.begin as u32, r.end as u32));
                            }
                        }
                    }
                    if ranges.is_empty() {
                        stack.push((depth, Scope::Skip));
                        continue;
                    }
                    let name = name_of(dwarf, unit, entry).unwrap_or_else(|| "?".to_string());
                    let linkage =
                        attr_str(dwarf, unit, entry, gimli::DW_AT_linkage_name).or_else(|| {
                            origin(unit, entry)
                                .and_then(|o| attr_str(dwarf, unit, &o, gimli::DW_AT_linkage_name))
                        });
                    let decl_line = attr_u64(entry, gimli::DW_AT_decl_line)
                        .or_else(|| {
                            origin(unit, entry).and_then(|o| attr_u64(&o, gimli::DW_AT_decl_line))
                        })
                        .unwrap_or(0) as u32;
                    let frame_base = match entry.attr_value(gimli::DW_AT_frame_base) {
                        Some(gimli::AttributeValue::Exprloc(e)) => Some(expr_from(e, enc)),
                        _ => None,
                    };
                    let low = ranges.iter().map(|r| r.0).min().unwrap_or(0);
                    let high = ranges.iter().map(|r| r.1).max().unwrap_or(0);
                    self.functions.push(Function {
                        name,
                        linkage_name: linkage,
                        low_pc: low,
                        high_pc: high,
                        decl_file: None,
                        decl_line,
                        frame_base,
                        vars: Vec::new(),
                    });
                    stack.push((depth, Scope::Function(self.functions.len() - 1, vec![])));
                }
                gimli::DW_TAG_lexical_block => {
                    let mut ranges = Vec::new();
                    if let Ok(mut it) = dwarf.die_ranges(unit, entry) {
                        while let Ok(Some(r)) = it.next() {
                            ranges.push((r.begin as u32, r.end as u32));
                        }
                    }
                    let scope = match parent {
                        Scope::Function(i, outer) => {
                            Scope::Function(i, if ranges.is_empty() { outer } else { ranges })
                        }
                        other => other,
                    };
                    stack.push((depth, scope));
                }
                gimli::DW_TAG_variable | gimli::DW_TAG_formal_parameter => {
                    if parent == Scope::Skip {
                        continue;
                    }
                    let is_param = entry.tag() == gimli::DW_TAG_formal_parameter;
                    let Some(name) = name_of(dwarf, unit, entry) else {
                        continue;
                    };
                    let location = self.location_of(dwarf, unit, entry, enc);
                    let ty_attr = entry.attr_value(gimli::DW_AT_type).or_else(|| {
                        origin(unit, entry).and_then(|o| o.attr_value(gimli::DW_AT_type))
                    });
                    let ty = ty_attr.and_then(|v| resolve_type(dwarf, unit, v, tcx, 0));
                    let decl_line = attr_u64(entry, gimli::DW_AT_decl_line).unwrap_or(0) as u32;
                    match parent {
                        Scope::Function(fi, scope) => {
                            self.functions[fi].vars.push(Variable {
                                name,
                                ty,
                                location,
                                is_param,
                                scope,
                                decl_line,
                            });
                        }
                        Scope::Global => {
                            let is_def = entry.attr_value(gimli::DW_AT_declaration).is_none();
                            if is_def && !matches!(location, VarLocation::OptimizedOut) && !is_param
                            {
                                self.globals.push(Variable {
                                    name,
                                    ty,
                                    location,
                                    is_param,
                                    scope: vec![],
                                    decl_line,
                                });
                            }
                        }
                        Scope::Skip => {}
                    }
                }
                _ => stack.push((depth, Scope::Skip)),
            }
        }
        Ok(())
    }

    fn location_of<'a>(
        &self,
        dwarf: &gimli::Dwarf<R<'a>>,
        unit: &gimli::Unit<R<'a>>,
        e: &gimli::DebuggingInformationEntry<R<'a>>,
        enc: gimli::Encoding,
    ) -> VarLocation {
        if let Some(v) = e.attr_value(gimli::DW_AT_location) {
            if let gimli::AttributeValue::Exprloc(x) = v {
                return VarLocation::Expr(expr_from(x, enc));
            }
            if let Ok(Some(mut it)) = dwarf.attr_locations(unit, v) {
                let mut list = Vec::new();
                while let Ok(Some(entry)) = it.next() {
                    list.push((
                        entry.range.begin as u32,
                        entry.range.end as u32,
                        expr_from(entry.data, enc),
                    ));
                }
                return VarLocation::List(list);
            }
        }
        if let Some(v) = e.attr_value(gimli::DW_AT_const_value)
            && let Some(c) = v
                .sdata_value()
                .or_else(|| v.udata_value().map(|u| u as i64))
        {
            return VarLocation::Const(c);
        }
        VarLocation::OptimizedOut
    }

    // ------------------------------------------------------------------ queries: lines

    /// Every source file mentioned in the line table.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    fn file_matches(&self, idx: u32, query: &str) -> bool {
        let p = &self.files[idx as usize];
        let q = Path::new(query);
        p == q
            || p.ends_with(q)
            || p.file_name()
                .is_some_and(|n| Some(n) == q.file_name() && q.components().count() == 1)
    }

    fn sequence_at(&self, pc: u32) -> Option<&Sequence> {
        let i = self.sequences.partition_point(|s| s.start <= pc);
        let s = self.sequences.get(i.checked_sub(1)?)?;
        (pc < s.end).then_some(s)
    }

    fn row_index(seq: &Sequence, pc: u32) -> Option<usize> {
        let i = seq.rows.partition_point(|r| r.addr <= pc);
        i.checked_sub(1)
    }

    /// Source location of a code address.
    pub fn location(&self, pc: u32) -> Option<SourceLocation> {
        let s = self.sequence_at(pc)?;
        let r = s.rows[Self::row_index(s, pc)?];
        Some(SourceLocation {
            file: self.files[r.file as usize].clone(),
            line: r.line,
            column: r.column,
        })
    }

    /// True if `pc` is the first address of a statement in the line table.
    pub fn is_statement_start(&self, pc: u32) -> bool {
        self.sequence_at(pc)
            .is_some_and(|s| s.rows.iter().any(|r| r.addr == pc && r.is_stmt))
    }

    /// Breakpoint addresses for `file:line`: the lowest statement address of that line in each
    /// function. If the line has no code, the next line with code (within 100 lines) is used.
    /// Returns (actual line, addresses).
    pub fn line_addresses(&self, file: &str, line: u32) -> Option<(u32, Vec<u32>)> {
        for l in line..line + 100 {
            let mut by_fn: HashMap<Option<usize>, u32> = HashMap::new();
            for s in &self.sequences {
                for r in &s.rows {
                    if r.line == l && r.is_stmt && self.file_matches(r.file, file) {
                        let f = self.function_index(r.addr);
                        let e = by_fn.entry(f).or_insert(r.addr);
                        *e = (*e).min(r.addr);
                    }
                }
            }
            if !by_fn.is_empty() {
                let mut v: Vec<u32> = by_fn.into_values().collect();
                v.sort();
                return Some((l, v));
            }
        }
        None
    }

    /// Address ranges of the line containing `pc`, within `pc`'s function (for stepping).
    pub fn line_ranges(&self, pc: u32) -> Vec<(u32, u32)> {
        let Some(s) = self.sequence_at(pc) else {
            return vec![];
        };
        let Some(i) = Self::row_index(s, pc) else {
            return vec![];
        };
        let cur = s.rows[i];
        let (lo, hi) = self
            .function_at(pc)
            .map(|f| (f.low_pc, f.high_pc))
            .unwrap_or((s.start, s.end));
        let mut out = Vec::new();
        for (k, r) in s.rows.iter().enumerate() {
            if r.file == cur.file && r.line == cur.line && r.addr >= lo && r.addr < hi {
                let end = s.rows.get(k + 1).map(|n| n.addr).unwrap_or(s.end).min(hi);
                if end > r.addr {
                    out.push((r.addr, end));
                }
            }
        }
        out
    }

    // ------------------------------------------------------------------ queries: functions

    fn function_index(&self, pc: u32) -> Option<usize> {
        // Innermost (smallest) function containing pc.
        self.functions
            .iter()
            .enumerate()
            .filter(|(_, f)| pc >= f.low_pc && pc < f.high_pc)
            .min_by_key(|(_, f)| f.high_pc - f.low_pc)
            .map(|(i, _)| i)
    }

    pub fn function_at(&self, pc: u32) -> Option<&Function> {
        self.function_index(pc).map(|i| &self.functions[i])
    }

    pub fn function_by_name(&self, name: &str) -> Option<&Function> {
        self.functions
            .iter()
            .find(|f| f.name == name || f.linkage_name.as_deref() == Some(name))
    }

    /// Name for a code address: DWARF function, else the nearest preceding function symbol.
    pub fn function_name(&self, pc: u32) -> Option<String> {
        if let Some(f) = self.function_at(pc) {
            return Some(f.name.clone());
        }
        self.symbols
            .iter()
            .filter(|s| s.is_function && s.addr <= pc && (s.size == 0 || pc < s.addr + s.size))
            .max_by_key(|s| s.addr)
            .map(|s| s.name.clone())
    }

    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.name == name)
    }

    pub fn global(&self, name: &str) -> Option<&Variable> {
        self.globals.iter().find(|g| g.name == name)
    }

    // ------------------------------------------------------------------ types

    pub fn type_name(&self, ty: Option<TypeId>) -> String {
        self.type_name_depth(ty, 0)
    }

    fn type_name_depth(&self, ty: Option<TypeId>, d: u32) -> String {
        if d > 8 {
            return "…".into();
        }
        let Some(t) = ty.and_then(|i| self.types.get(i)) else {
            return "void".into();
        };
        match t {
            TypeInfo::Base { name, .. } | TypeInfo::Typedef { name, .. } => name.clone(),
            TypeInfo::Pointer { target, .. } => {
                format!("{}*", self.type_name_depth(*target, d + 1))
            }
            TypeInfo::Array { elem, count } => match count {
                Some(c) => format!("{}[{c}]", self.type_name_depth(*elem, d + 1)),
                None => format!("{}[]", self.type_name_depth(*elem, d + 1)),
            },
            TypeInfo::Struct { name, .. } => name.clone(),
            TypeInfo::Enum { name, .. } => name.clone(),
            TypeInfo::Qualified { qual, target } => {
                format!("{qual} {}", self.type_name_depth(*target, d + 1))
            }
            TypeInfo::Void => "void".into(),
        }
    }

    /// Strips typedefs and qualifiers.
    fn strip(&self, mut ty: Option<TypeId>) -> Option<&TypeInfo> {
        for _ in 0..16 {
            match ty.and_then(|i| self.types.get(i)) {
                Some(TypeInfo::Typedef { target, .. })
                | Some(TypeInfo::Qualified { target, .. }) => ty = *target,
                other => return other,
            }
        }
        None
    }

    pub fn type_size(&self, ty: Option<TypeId>) -> u32 {
        match self.strip(ty) {
            Some(TypeInfo::Base { size, .. }) | Some(TypeInfo::Pointer { size, .. }) => *size,
            Some(TypeInfo::Struct { size, .. }) | Some(TypeInfo::Enum { size, .. }) => *size,
            Some(TypeInfo::Array { elem, count }) => self.type_size(*elem) * count.unwrap_or(0),
            _ => 0,
        }
    }

    // ------------------------------------------------------------------ CFI

    /// CFA and caller register recovery from `.debug_frame`.
    /// Returns (cfa, rules) where rules are (dwarf register, offset from CFA).
    pub fn cfi_at(&self, pc: u32, regs: &FrameRegs) -> Option<(u32, Vec<(u16, i64)>)> {
        if self.debug_frame.is_empty() {
            return None;
        }
        let mut df = gimli::DebugFrame::new(&self.debug_frame, LittleEndian);
        df.set_address_size(4);
        let bases = gimli::BaseAddresses::default();
        let mut ctx = Box::new(gimli::UnwindContext::new());
        let row = df
            .unwind_info_for_address(
                &bases,
                &mut ctx,
                pc as u64,
                gimli::DebugFrame::cie_from_offset,
            )
            .ok()?;
        let cfa = match row.cfa() {
            gimli::CfaRule::RegisterAndOffset { register, offset } => {
                let base = match register.0 {
                    32 => regs.sp,
                    26 | 28 | 30 => regs.pair(register.0 as usize),
                    r if r < 32 => regs.regs[r as usize] as u32,
                    _ => return None,
                };
                (base as i64 + offset) as u32
            }
            _ => return None,
        };
        let mut rules = Vec::new();
        for (reg, rule) in row.registers() {
            if let gimli::RegisterRule::Offset(o) = rule {
                rules.push((reg.0, *o));
            }
        }
        Some((cfa, rules))
    }

    /// Unwinds one frame with CFI: returns the caller's registers.
    pub fn unwind(&self, avr: &Avr, regs: &FrameRegs) -> Option<FrameRegs> {
        let (cfa, rules) = self.cfi_at(regs.pc, regs)?;
        let mut caller = FrameRegs {
            pc: 0,
            regs: regs.regs,
            known: 0,
            sp: cfa,
            cfa: None,
        };
        let mut ra = None;
        for (reg, off) in rules {
            let at = (cfa as i64 + off) as u32;
            if reg == 36 {
                let mut v = 0u32;
                for i in 0..self.pc_bytes {
                    v = (v << 8) | avr.peek((at + i) as u16) as u32;
                }
                ra = Some(v * 2);
            } else if reg < 32 {
                caller.regs[reg as usize] = avr.peek(at as u16);
                caller.known |= 1 << reg;
            }
        }
        caller.pc = ra?;
        Some(caller)
    }

    // ------------------------------------------------------------------ evaluation

    fn eval_pieces<'e>(
        &self,
        expr: &'e Expr,
        avr: &Avr,
        fr: &FrameRegs,
        frame_base: Option<u32>,
    ) -> Result<Vec<gimli::Piece<R<'e>>>, String> {
        let mut ev =
            gimli::Evaluation::new(EndianSlice::new(&expr.bytes, LittleEndian), expr.encoding);
        let mut res = ev.evaluate().map_err(|e| e.to_string())?;
        loop {
            res = match res {
                gimli::EvaluationResult::Complete => break,
                gimli::EvaluationResult::RequiresMemory { address, size, .. } => {
                    let a = data_addr(address);
                    let mut v = 0u64;
                    for i in (0..size as u32).rev() {
                        v = (v << 8) | avr.peek((a + i) as u16) as u64;
                    }
                    ev.resume_with_memory(gimli::Value::Generic(v))
                        .map_err(|e| e.to_string())?
                }
                gimli::EvaluationResult::RequiresRegister { register, .. } => {
                    let r = register.0 as usize;
                    let v = match r {
                        32 => fr.sp,
                        26 | 28 | 30 => fr.pair(r),
                        r if r < 32 => fr.regs[r] as u32,
                        _ => return Err(format!("register {r}")),
                    };
                    ev.resume_with_register(gimli::Value::Generic(v as u64))
                        .map_err(|e| e.to_string())?
                }
                gimli::EvaluationResult::RequiresFrameBase => {
                    let fb = frame_base.ok_or("no frame base")?;
                    ev.resume_with_frame_base(fb as u64)
                        .map_err(|e| e.to_string())?
                }
                gimli::EvaluationResult::RequiresCallFrameCfa => {
                    let cfa = fr.cfa.ok_or("no CFA")?;
                    ev.resume_with_call_frame_cfa(cfa as u64)
                        .map_err(|e| e.to_string())?
                }
                gimli::EvaluationResult::RequiresRelocatedAddress(a) => ev
                    .resume_with_relocated_address(a)
                    .map_err(|e| e.to_string())?,
                other => return Err(format!("unsupported DWARF operation ({other:?})")),
            };
        }
        Ok(ev.result())
    }

    /// Frame base of `f` in a frame (DW_AT_frame_base, usually DW_OP_call_frame_cfa).
    pub fn frame_base(&self, f: &Function, avr: &Avr, fr: &FrameRegs) -> Option<u32> {
        let fb = f.frame_base.as_ref()?;
        let pieces = self.eval_pieces(fb, avr, fr, None).ok()?;
        match pieces.first()?.location {
            gimli::Location::Address { address } => Some(address as u32),
            gimli::Location::Value { value } => value.to_u64(u64::MAX).ok().map(|v| v as u32),
            gimli::Location::Register { register } => match register.0 {
                32 => Some(fr.sp),
                r @ (26 | 28 | 30) => Some(fr.pair(r as usize)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Reads `var` in the given frame.
    pub fn read_variable(
        &self,
        avr: &Avr,
        var: &Variable,
        fr: &FrameRegs,
        frame_base: Option<u32>,
    ) -> VarValue {
        let size = self.type_size(var.ty).max(1);
        let type_name = self.type_name(var.ty);
        let mk = |value: Value, address: Option<u32>, location: String| VarValue {
            name: var.name.clone(),
            type_name: type_name.clone(),
            value,
            address,
            location,
            is_param: var.is_param,
        };
        let expr = match &var.location {
            VarLocation::Const(c) => {
                let v = self.decode(
                    avr,
                    var.ty,
                    &c.to_le_bytes()[..size.min(8) as usize],
                    None,
                    0,
                );
                return mk(v, None, "const".into());
            }
            VarLocation::OptimizedOut => {
                return mk(Value::OptimizedOut, None, "optimized out".into());
            }
            VarLocation::Expr(e) => e,
            VarLocation::List(l) => match l.iter().find(|(b, e, _)| fr.pc >= *b && fr.pc < *e) {
                Some((_, _, e)) => e,
                None => return mk(Value::OptimizedOut, None, "optimized out".into()),
            },
        };
        let pieces = match self.eval_pieces(expr, avr, fr, frame_base) {
            Ok(p) => p,
            Err(e) => return mk(Value::Unavailable(e), None, "?".into()),
        };
        let mut bytes: Vec<u8> = Vec::new();
        let mut where_: Vec<String> = Vec::new();
        let mut address = None;
        let mut missing = false;
        let single = pieces.len() == 1;
        for p in &pieces {
            let n = p
                .size_in_bits
                .map(|b| b.div_ceil(8) as u32)
                .unwrap_or(if single { size } else { 1 });
            match p.location {
                gimli::Location::Empty => {
                    missing = true;
                    bytes.extend(std::iter::repeat_n(0, n as usize));
                }
                gimli::Location::Register { register } => {
                    let r = register.0 as usize;
                    for i in 0..n as usize {
                        let rr = r + i;
                        if rr < 32 {
                            if fr.known & (1 << rr) == 0 {
                                missing = true;
                            }
                            bytes.push(fr.regs[rr]);
                            where_.push(format!("r{rr}"));
                        } else {
                            missing = true;
                            bytes.push(0);
                        }
                    }
                }
                gimli::Location::Address { address: raw } => {
                    let eeprom = (0x81_0000..0x82_0000).contains(&raw);
                    let a = data_addr(raw);
                    if address.is_none() && !eeprom {
                        address = Some(a);
                    }
                    for i in 0..n as u64 {
                        bytes.push(peek_any(avr, raw + i));
                    }
                    where_.push(if eeprom {
                        format!("eeprom 0x{:03x}", raw - 0x81_0000)
                    } else {
                        format!("0x{a:04x}")
                    });
                }
                gimli::Location::Value { value } => {
                    let v = value.to_u64(u64::MAX).unwrap_or(0);
                    bytes.extend_from_slice(&v.to_le_bytes()[..n.min(8) as usize]);
                    where_.push("value".into());
                }
                _ => {
                    missing = true;
                    bytes.extend(std::iter::repeat_n(0, n as usize));
                }
            }
        }
        if missing && bytes.iter().all(|b| *b == 0) && address.is_none() {
            return mk(Value::OptimizedOut, None, "optimized out".into());
        }
        let loc = if where_.iter().all(|w| w.starts_with('r')) && !where_.is_empty() {
            where_.join(":")
        } else {
            where_.first().cloned().unwrap_or_default()
        };
        let v = self.decode(avr, var.ty, &bytes, address, 0);
        mk(v, address, loc)
    }

    /// Decodes little-endian `bytes` as `ty` (arrays/structs read memory from `addr`).
    fn decode(
        &self,
        avr: &Avr,
        ty: Option<TypeId>,
        bytes: &[u8],
        addr: Option<u32>,
        depth: u32,
    ) -> Value {
        if depth > 6 {
            return Value::Unavailable("too deep".into());
        }
        let le = |b: &[u8]| -> u64 { b.iter().rev().fold(0u64, |a, x| (a << 8) | *x as u64) };
        match self.strip(ty) {
            None | Some(TypeInfo::Void) => Value::UInt(le(bytes)),
            Some(TypeInfo::Base {
                size,
                encoding,
                name,
            }) => {
                let n = (*size as usize).min(bytes.len()).min(8);
                let raw = le(&bytes[..n]);
                match *encoding {
                    0x02 => Value::Bool(raw != 0),
                    0x04 => {
                        if n == 4 {
                            Value::Float(f32::from_bits(raw as u32) as f64)
                        } else if n == 8 {
                            Value::Float(f64::from_bits(raw))
                        } else {
                            Value::Unavailable(format!("{name}"))
                        }
                    }
                    0x05 => {
                        let shift = 64 - 8 * n as u32;
                        Value::Int(((raw << shift) as i64) >> shift)
                    }
                    0x06 | 0x08 if n == 1 => Value::Char(raw as u8),
                    0x06 => Value::Int(raw as i8 as i64),
                    _ => Value::UInt(raw),
                }
            }
            Some(TypeInfo::Pointer { target, size }) => {
                let n = (*size as usize).min(bytes.len());
                let a = le(&bytes[..n]) as u32;
                let is_char = matches!(
                    self.strip(*target),
                    Some(TypeInfo::Base {
                        size: 1,
                        encoding: 0x06 | 0x08,
                        ..
                    })
                );
                let string = is_char.then(|| {
                    let mut s = String::new();
                    for i in 0..32u32 {
                        let c = avr.peek((a + i) as u16);
                        if c == 0 {
                            break;
                        }
                        s.push(if c.is_ascii() { c as char } else { '?' });
                    }
                    s
                });
                Value::Pointer { addr: a, string }
            }
            Some(TypeInfo::Array { elem, count }) => {
                let es = self.type_size(*elem).max(1);
                let n = count.unwrap_or(0).min(64);
                let mut v = Vec::new();
                for i in 0..n {
                    let eb: Vec<u8> = match addr {
                        Some(a) => (0..es).map(|k| avr.peek((a + i * es + k) as u16)).collect(),
                        None => bytes
                            .get((i * es) as usize..((i + 1) * es) as usize)
                            .map(|s| s.to_vec())
                            .unwrap_or_default(),
                    };
                    v.push(self.decode(avr, *elem, &eb, addr.map(|a| a + i * es), depth + 1));
                }
                Value::Array(v)
            }
            Some(TypeInfo::Struct { members, .. }) => {
                let mut v = Vec::new();
                for (name, off, mty) in members {
                    let ms = self.type_size(*mty).max(1);
                    let mb: Vec<u8> = match addr {
                        Some(a) => (0..ms).map(|k| avr.peek((a + off + k) as u16)).collect(),
                        None => bytes
                            .get(*off as usize..(off + ms) as usize)
                            .map(|s| s.to_vec())
                            .unwrap_or_default(),
                    };
                    v.push((
                        name.clone(),
                        self.decode(avr, *mty, &mb, addr.map(|a| a + off), depth + 1),
                    ));
                }
                Value::Struct(v)
            }
            Some(TypeInfo::Enum { size, values, .. }) => {
                let n = (*size as usize).min(bytes.len());
                let raw = le(&bytes[..n]) as i64;
                let name = values
                    .iter()
                    .find(|(_, x)| *x == raw)
                    .map(|(n, _)| n.clone());
                Value::Enum { value: raw, name }
            }
            Some(TypeInfo::Typedef { .. }) | Some(TypeInfo::Qualified { .. }) => {
                Value::Unavailable("type".into())
            }
        }
    }
}

fn resolve_type<'a>(
    dwarf: &gimli::Dwarf<R<'a>>,
    unit: &gimli::Unit<R<'a>>,
    v: gimli::AttributeValue<R<'a>>,
    tcx: &mut TypeCtx<'a>,
    depth: u32,
) -> Option<TypeId> {
    if depth > 24 {
        return None;
    }
    let off = match v {
        gimli::AttributeValue::UnitRef(o) => o,
        _ => return None,
    };
    let key = off
        .to_debug_info_offset(&unit.header)
        .map(|o| o.0)
        .unwrap_or(off.0);
    if let Some(id) = tcx.map.get(&key) {
        return Some(*id);
    }
    let e = unit.entry(off).ok()?;
    let id = tcx.types.len();
    tcx.types.push(TypeInfo::Void);
    tcx.map.insert(key, id);
    let name = attr_str(dwarf, unit, &e, gimli::DW_AT_name);
    let size = attr_u64(&e, gimli::DW_AT_byte_size).unwrap_or(0) as u32;
    let target = e
        .attr_value(gimli::DW_AT_type)
        .and_then(|t| resolve_type(dwarf, unit, t, tcx, depth + 1));
    let t = match e.tag() {
        gimli::DW_TAG_base_type => TypeInfo::Base {
            name: name.unwrap_or_else(|| "?".into()),
            size,
            encoding: match e.attr_value(gimli::DW_AT_encoding) {
                Some(gimli::AttributeValue::Encoding(x)) => x.0,
                _ => 0,
            },
        },
        gimli::DW_TAG_pointer_type
        | gimli::DW_TAG_reference_type
        | gimli::DW_TAG_rvalue_reference_type => TypeInfo::Pointer {
            target,
            size: if size == 0 { 2 } else { size },
        },
        gimli::DW_TAG_typedef => TypeInfo::Typedef {
            name: name.unwrap_or_else(|| "?".into()),
            target,
        },
        gimli::DW_TAG_const_type => TypeInfo::Qualified {
            qual: "const",
            target,
        },
        gimli::DW_TAG_volatile_type => TypeInfo::Qualified {
            qual: "volatile",
            target,
        },
        gimli::DW_TAG_array_type => {
            let mut count = None;
            if let Ok(mut tree) = unit.entries_tree(Some(off))
                && let Ok(root) = tree.root()
            {
                let mut children = root.children();
                while let Ok(Some(child)) = children.next() {
                    let c = child.entry();
                    if c.tag() == gimli::DW_TAG_subrange_type {
                        count = attr_u64(c, gimli::DW_AT_count)
                            .map(|n| n as u32)
                            .or_else(|| {
                                attr_u64(c, gimli::DW_AT_upper_bound).map(|u| u as u32 + 1)
                            });
                        break;
                    }
                }
            }
            TypeInfo::Array {
                elem: target,
                count,
            }
        }
        gimli::DW_TAG_structure_type | gimli::DW_TAG_class_type | gimli::DW_TAG_union_type => {
            let mut members = Vec::new();
            if let Ok(mut tree) = unit.entries_tree(Some(off))
                && let Ok(root) = tree.root()
            {
                let mut children = root.children();
                while let Ok(Some(child)) = children.next() {
                    let c = child.entry().clone();
                    if c.tag() == gimli::DW_TAG_member
                        && c.attr_value(gimli::DW_AT_external).is_none()
                    {
                        let mname =
                            attr_str(dwarf, unit, &c, gimli::DW_AT_name).unwrap_or_default();
                        let moff =
                            attr_u64(&c, gimli::DW_AT_data_member_location).unwrap_or(0) as u32;
                        let mty = c.attr_value(gimli::DW_AT_type);
                        members.push((mname, moff, mty));
                    }
                }
            }
            let members = members
                .into_iter()
                .map(|(n, o, t)| {
                    (
                        n,
                        o,
                        t.and_then(|t| resolve_type(dwarf, unit, t, tcx, depth + 1)),
                    )
                })
                .collect();
            TypeInfo::Struct {
                name: name.unwrap_or_else(|| "struct".into()),
                size,
                members,
            }
        }
        gimli::DW_TAG_enumeration_type => {
            let mut values = Vec::new();
            if let Ok(mut tree) = unit.entries_tree(Some(off))
                && let Ok(root) = tree.root()
            {
                let mut children = root.children();
                while let Ok(Some(child)) = children.next() {
                    let c = child.entry();
                    if c.tag() == gimli::DW_TAG_enumerator {
                        let n = attr_str(dwarf, unit, c, gimli::DW_AT_name).unwrap_or_default();
                        let v = c
                            .attr_value(gimli::DW_AT_const_value)
                            .and_then(|v| {
                                v.sdata_value()
                                    .or_else(|| v.udata_value().map(|u| u as i64))
                            })
                            .unwrap_or(0);
                        values.push((n, v));
                    }
                }
            }
            TypeInfo::Enum {
                name: name.unwrap_or_else(|| "enum".into()),
                size,
                values,
            }
        }
        _ => TypeInfo::Typedef {
            name: name.unwrap_or_else(|| "?".into()),
            target,
        },
    };
    tcx.types[id] = t;
    Some(id)
}
