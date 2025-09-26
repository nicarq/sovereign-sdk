// Simple WASM program for Ligetron that reads arguments and emits journals

use std::vec::Vec;

fn main() {
    // Read arguments using WASI
    let args = read_wasi_args();
    
    // Create a simple deterministic journal based on the number of arguments
    let journal = if args.len() >= 2 {
        format!("ligetron_journal_args_{}", args.len()).into_bytes()
    } else {
        b"ligetron_journal_no_args".to_vec()
    };
    
    // Emit journal in the expected format
    emit_journal(&journal);
}

fn read_wasi_args() -> Vec<Vec<u8>> {
    extern "C" {
        fn args_sizes_get(argc: *mut usize, argv_buf_size: *mut usize) -> u16;
        fn args_get(argv: *mut *mut u8, argv_buf: *mut u8) -> u16;
    }

    unsafe {
        let mut argc: usize = 0;
        let mut argv_buf_size: usize = 0;

        // Get sizes
        if args_sizes_get(&mut argc, &mut argv_buf_size) != 0 {
            return Vec::new();
        }

        if argc == 0 || argv_buf_size == 0 {
            return Vec::new();
        }

        // Allocate buffers
        let mut argv = vec![std::ptr::null_mut::<u8>(); argc];
        let mut argv_buf = vec![0u8; argv_buf_size];

        // Get arguments
        if args_get(argv.as_mut_ptr(), argv_buf.as_mut_ptr()) != 0 {
            return Vec::new();
        }

        // Extract arguments
        let mut result = Vec::new();
        let base_ptr = argv_buf.as_ptr() as usize;

        for i in 0..argc {
            let arg_ptr = argv[i];
            if arg_ptr.is_null() {
                continue;
            }

            let offset = (arg_ptr as usize).saturating_sub(base_ptr);
            if offset >= argv_buf_size {
                continue;
            }

            // Find the null terminator
            let mut len = 0;
            while offset + len < argv_buf_size && argv_buf[offset + len] != 0 {
                len += 1;
            }

            if len > 0 {
                result.push(argv_buf[offset..offset + len].to_vec());
            }
        }

        result
    }
}

fn emit_journal(data: &[u8]) {
    extern "C" {
        fn fd_write(fd: u32, iovs: *const IoVec, iovs_len: usize, nwritten: *mut u32) -> u16;
    }

    #[repr(C)]
    struct IoVec {
        ptr: *const u8,
        len: usize,
    }

    // Create the SOV_JOURNAL_HEX output format that the prover expects
    let journal_line = format!("SOV_JOURNAL_HEX:{}\n", hex::encode(data));
    let iov = IoVec {
        ptr: journal_line.as_ptr(),
        len: journal_line.len(),
    };
    let mut nw: u32 = 0;
    unsafe {
        fd_write(1, &iov, 1, &mut nw); // Write to stdout
    }
}