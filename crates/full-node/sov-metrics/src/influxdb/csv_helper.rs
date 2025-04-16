use std::fs::File;
use std::io::{self, BufWriter, Write};

/// Allows writeing metrics to CSV file.
pub struct CsvWriteres {
    pub(crate) constatn_writer: BufWriter<File>,
    pub(crate) zk_vm_writer: BufWriter<File>,
}

fn create_writer(file_name: &str, header: &str) -> io::Result<BufWriter<File>> {
    let mut writer = BufWriter::new(File::create(file_name)?);
    writer.write_all(header.as_bytes())?;
    Ok(writer)
}

impl CsvWriteres {
    pub(crate) async fn new() -> io::Result<Self> {
        let constatn_writer = create_writer(
            "constnats_output.csv",
            "name,constnat,num_invocations,pre_state_root\n",
        )?;

        let zk_vm_writer = create_writer(
            "zk_vm.csv",
            "name,cycles_count,memory_used,free_heap_bytes,pre_state_root\n",
        )?;

        Ok(Self {
            constatn_writer,
            zk_vm_writer,
        })
    }
}
