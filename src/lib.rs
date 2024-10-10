use std::path::{PathBuf, Path};

use elf::ElfBytes;
use elf::endian::AnyEndian;
use elf::abi::{DT_NEEDED, DT_RUNPATH, DT_RPATH};
use elf::file::Class::*;
use std::env;
use std::fs;

pub struct ElfFile {
    path: PathBuf
}

impl ElfFile {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref().to_owned();
        Self {
            path
        }
    }

    pub fn get_libs_full_paths(&self) -> Option<Vec<PathBuf>> {
        let mut libs = Vec::new();
        let mut lib_paths = Vec::new();

        // Parse first argument and create ElfBytes object from it.
        let elf_file_name = env::args().nth(1).expect("First argument must be the path (relative or absolute) to an ELF file");
        let path = PathBuf::from(&elf_file_name);
        let elf_file_data = fs::read(&path).expect("Could not read ELF file with std::fs::read");
        let elf = ElfBytes::<AnyEndian>::minimal_parse(elf_file_data.as_slice()).expect("ElfBytes::minimal_parse failed");

        // Determine if ELF file is 32-bit or 64-bit
        let is_64_bit = match elf.ehdr.class {
            ELF64 => true,
            ELF32 => false,
        };

        // First, get the slice of bytes for the `.dynstr` section (which `.dynamic` will index)
        let elf_dynstr_header = elf.section_header_by_name(".dynstr").expect("Error parsing ELF header").expect("The ELF file should have a \".dynstr\" section");
        let dynstr_offset = elf_dynstr_header.sh_offset as usize;
        let dynstr_size = elf_dynstr_header.sh_size as usize;
        let dynstr_bytes = &elf_file_data[dynstr_offset..(dynstr_offset + dynstr_size)];

        // Directories in which to search for libraries
        let mut search_dirs: Vec<PathBuf> = [
            "/usr/lib",
            "/lib64",
            "/lib/x86_64-linux-gnu",
            "/lib",
            "/usr/lib64"
        ].into_iter().map(PathBuf::from).collect();

        // Get the `.dynamic` section and process DT_NEEDED libraries
        let dynamic = elf.dynamic().expect("Failed to parse ELF header!").expect("ELF header must have a \".dynamic\" section!");
        for entry in dynamic {
            match entry.d_tag {
                DT_NEEDED => {
                    // This is a needed shared library!
                    let offset = entry.d_val() as usize;
                    libs.push(u8_slice_to_str(&dynstr_bytes[offset..])?.to_owned());
                }
                DT_RPATH | DT_RUNPATH => {
                    let offset = entry.d_val() as usize;
                    let paths_str = u8_slice_to_str(&dynstr_bytes[offset..]).expect("Invalid RPATH/RUNPATH string");
                    for path in paths_str.split(':') {
                        search_dirs.push(PathBuf::from(path));
                    }
                }
                _ => ()
            }
        }

        for lib in libs.iter() {
            let mut found = false;
            for dir in search_dirs.iter() {
                let possible_lib_path = dir.join(lib);
                if possible_lib_path.exists() {
                    if verify_arch(&possible_lib_path, is_64_bit) {
                        lib_paths.push(possible_lib_path);
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                // Failed to find `lib` anywhere!
                return None;
            }
        }

        Some(lib_paths)
    }
}

fn u8_slice_to_str(c_str: &[u8]) -> Option<&str> {
    // Find null terminator
    if let Some(end) = c_str[0..].iter().position(|&b| b == b'\0') {

        // Create c string slice
        let slice = &c_str[0..end];

        std::str::from_utf8(slice).ok()
    } else {
        None
    }
}

fn verify_arch(lib_path: &Path, is_64_bit_executable: bool) -> bool {
    if let Ok(lib_data) = fs::read(lib_path) {
        if let Ok(lib_elf) = ElfBytes::<AnyEndian>::minimal_parse(lib_data.as_slice()) {
            let lib_header = lib_elf.ehdr;
            match (lib_header.class, is_64_bit_executable) {
                (ELF64, true) => true,
                (ELF32, false) => true,
                _ => false,
            }
        } else {
            false
        }
    } else {
        false
    }
}

