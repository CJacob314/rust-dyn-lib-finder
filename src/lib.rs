//! # What is this?
//! This is a library crate designed to function like `ldd`.
use std::path::{PathBuf, Path};

use elf::ElfBytes;
use elf::endian::AnyEndian;
use elf::abi::{DT_NEEDED, DT_RUNPATH, DT_RPATH};
use elf::file::Class::*;
use std::fs;
use std::collections::HashSet;
use std::env;

/// Represents an ELF file on disk and provides the method [`ElfFile::get_libs_full_paths`] to
/// recursively get ELF-header-declared shared-library dependencies.
pub struct ElfFile {
    path: PathBuf,
}

impl ElfFile {
    /// Creates an [`ElfFile`] instance from [`AsRef<Path>`]
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref().to_owned();
        Self { path }
    }

    /// # Return Value [None]
    /// A return value of [`Option::None`] means some library was not found for some reason 
    /// ## TODO: Make return type `Vec<(String, Result<PathBuf, CustomErrorTypeOrJustStr>)>`).
    /// # Return Value [Some]
    /// The value contained in the returned [`Option::Some`] is a vector of the paths to all found
    /// shared-library dependencies on disk.
    /// # Paths Searched
    /// - All valid directories in `LD_LIBRARY_PATH` environment variable
    /// - ELF `RPATH`
    /// - ELF `RUNPATH`
    /// - `/usr/lib`
    /// - `/lib64`
    /// - `/lib/x86_64-linux-gnu`
    /// - `/lib`
    /// - `/usr/lib64`
    pub fn get_libs_full_paths(&self) -> Option<Vec<PathBuf>> {
        let mut seen_libs = HashSet::new();
        let mut lib_paths = Vec::new();
        // Add the initial path to seen_libs
        seen_libs.insert(self.path.clone());
        ElfFile::collect_libs(&self.path, &mut seen_libs, &mut lib_paths)
            .map(|_| lib_paths)
    }

    fn collect_libs(
        path: &Path,
        seen_libs: &mut HashSet<PathBuf>,
        lib_paths: &mut Vec<PathBuf>,
    ) -> Option<()> {
        // Read the ELF file
        let elf_file_data = fs::read(path).expect("Could not read ELF file with std::fs::read");
        let elf = ElfBytes::<AnyEndian>::minimal_parse(elf_file_data.as_slice())
            .expect("ElfBytes::minimal_parse failed");

        // Determine if ELF file is 32-bit or 64-bit
        let is_64_bit = match elf.ehdr.class {
            ELF64 => true,
            ELF32 => false,
        };

        // First, get the slice of bytes for the `.dynstr` section (which `.dynamic` will index)
        let elf_dynstr_header = elf
            .section_header_by_name(".dynstr")
            .expect("Error parsing ELF header")
            .expect("The ELF file should have a \".dynstr\" section");
        let dynstr_offset = elf_dynstr_header.sh_offset as usize;
        let dynstr_size = elf_dynstr_header.sh_size as usize;
        let dynstr_bytes = &elf_file_data[dynstr_offset..(dynstr_offset + dynstr_size)];

        // Directories in which to search for libraries
        let mut search_dirs: Vec<PathBuf> = [
            "/usr/lib",
            "/lib64",
            "/lib/x86_64-linux-gnu",
            "/lib",
            "/usr/lib64",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();

        if let Ok(ld_library_path_var) = env::var("LD_LIBRARY_PATH") {
            for lib_path_str in ld_library_path_var.split(':') {
                let lib_path = PathBuf::from(lib_path_str);
                if lib_path.exists() {
                    search_dirs.push(lib_path);
                }
            }
        }

        // Get the `.dynamic` section and process DT_NEEDED libraries
        let mut libs = Vec::new();
        let dynamic = elf
            .dynamic()
            .expect("Failed to parse ELF header!")
            .expect("ELF header must have a \".dynamic\" section!");
        for entry in dynamic {
            match entry.d_tag {
                DT_NEEDED => {
                    // This is a needed shared library!
                    let offset = entry.d_val() as usize;
                    libs.push(u8_slice_to_str(&dynstr_bytes[offset..])?.to_owned());
                }
                DT_RPATH | DT_RUNPATH => {
                    let offset = entry.d_val() as usize;
                    let paths_str =
                        u8_slice_to_str(&dynstr_bytes[offset..]).expect("Invalid RPATH/RUNPATH string");
                    for path in paths_str.split(':') {
                        search_dirs.push(PathBuf::from(path));
                    }
                }
                _ => (),
            }
        }

        for lib in libs.iter() {
            let mut found = false;
            for dir in search_dirs.iter() {
                let possible_lib_path = dir.join(lib);
                if possible_lib_path.exists() && verify_arch(&possible_lib_path, is_64_bit) {
                    // Check if we've already processed this library
                    if seen_libs.contains(&possible_lib_path) {
                        found = true;
                        break;
                    }
                    // Add to seen_libs
                    seen_libs.insert(possible_lib_path.clone());
                    lib_paths.push(possible_lib_path.clone());
                    found = true;
                    // Recurse into the library
                    ElfFile::collect_libs(&possible_lib_path, seen_libs, lib_paths)?;
                    break;
                }
            }
            if !found {
                // Failed to find `lib` anywhere!
                return None;
            }
        }
        Some(())
    }
}

fn u8_slice_to_str(c_str: &[u8]) -> Option<&str> {
    // Find null terminator
    if let Some(end) = c_str.iter().position(|&b| b == b'\0') {
        // Create c string slice
        let slice = &c_str[..end];
        std::str::from_utf8(slice).ok()
    } else {
        None
    }
}

fn verify_arch(lib_path: &Path, is_64_bit_executable: bool) -> bool {
    if let Ok(lib_data) = fs::read(lib_path) {
        if let Ok(lib_elf) = ElfBytes::<AnyEndian>::minimal_parse(lib_data.as_slice()) {
            let lib_header = lib_elf.ehdr;
            matches!((lib_header.class, is_64_bit_executable), (ELF64, true) | (ELF32, false))
        } else {
            false
        }
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::ElfFile;
    use std::path::PathBuf;

    #[test]
    fn test_libc_dependencies() {
        let files = [
            "/usr/bin/grep",
            "/usr/bin/echo",
            "/usr/bin/ls"
        ];
        for file in files {
            test_libc_dependency(file);
        }
    }

    fn test_libc_dependency(elf_file_path: &str) {
        let elf_file = ElfFile::new(elf_file_path);
        let libs = elf_file
            .get_libs_full_paths()
            .expect("Failed to get dependencies");

        let libc_path = PathBuf::from("/lib/x86_64-linux-gnu/libc.so.6");
        assert!(
            libs.contains(&libc_path),
            "Expected dependency {libc_path:?} not found in dependencies of \"{elf_file_path}\"",
        );
    }
}
