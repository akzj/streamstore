use std::{fs::File, io::Write};

use crate::{error::Error, mem_table::MemTable};

const SEGMENT_STREAM_HEADER_SIZE: u64 = std::mem::size_of::<SegmentStreamHeader>() as u64;
const SEGMENT_HEADER_SIZE: u64 = std::mem::size_of::<SegmentHeader>() as u64;
const SEGMENT_STREAM_HEADER_VERSION_V1: u64 = 1;
const SEGMENT_HEADER_VERSION_V1: u64 = 1;

#[derive(Default, Debug, Clone)]
#[repr(C)]
pub struct SegmentStreamHeader {
    pub version: u64,
    // The stream id
    pub stream_id: u64,
    // The offset of the stream
    pub offset: u64,
    // The offset of the stream in the file
    pub file_offset: u64,
    // The size of the stream in the file
    pub size: u64,
    // checksum of the stream
    pub crc_check_sum: u64,
}

#[derive(Default, Clone, Debug)]
#[repr(C)]
pub struct SegmentHeader {
    version: u64,
    last_entry: u64,
    first_entry: u64,
    stream_headers_offset: u64,
    stream_headers_count: u64,
}

pub struct Segment {
    file: File,
    pub file_name: String,
    data: memmap2::Mmap,
}

impl Segment {
    pub fn new(file: File, file_name: String, data: memmap2::Mmap) -> Self {
        Segment {
            file,
            data,
            file_name,
        }
    }

    pub fn open(file_name: &str) -> Result<Segment, Error> {
        let file = File::open(file_name).map_err(Error::new_io_error)?;
        let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(Error::new_io_error)?;
        let segment = Segment {
            file,
            file_name: file_name.to_string(),
            data: mmap,
        };
        Ok(segment)
    }

    pub fn entry_index(&self) -> (u64, u64) {
        let header = self.read_header();
        (header.first_entry, header.last_entry)
    }

    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    pub fn read_header(&self) -> SegmentHeader {
        unsafe { &*(self.data.as_ptr() as *const SegmentHeader) }.clone()
    }

    fn stream_headers(&self) -> &[SegmentStreamHeader] {
        let header = self.read_header();
        unsafe {
            std::slice::from_raw_parts(
                self.data
                    .as_ptr()
                    .add(header.stream_headers_offset as usize)
                    as *const SegmentStreamHeader,
                header.stream_headers_count as usize,
            )
        }
    }

    pub fn get_stream_range(&self, stream_id: u64) -> Option<(u64, u64)> {
        let stream_header = self.find_stream_header(stream_id)?;
        let offset = stream_header.file_offset;
        let size = stream_header.size;
        Some((offset, offset + size))
    }

    pub fn find_stream_header(&self, stream_id: u64) -> Option<SegmentStreamHeader> {
        self.stream_headers()
            .binary_search_by(|b| {
                if b.stream_id < stream_id {
                    std::cmp::Ordering::Less
                } else if b.stream_id > stream_id {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()
            .map(|index| self.stream_headers()[index].clone())
    }

    pub fn stream_data(&self, stream_id: u64) -> Option<&[u8]> {
        let stream_header = self.find_stream_header(stream_id)?;
        let offset = stream_header.file_offset;
        let size = stream_header.size;

        let data = unsafe {
            std::slice::from_raw_parts(
                self.data.as_ptr().add(offset as usize) as *const u8,
                size as usize,
            )
        };
        Some(data)
    }
}

pub fn generate_segment(filename: &str, table: &MemTable) -> Result<Segment, Error> {
    assert!(align_of::<SegmentHeader>() <= 8);

    let temp_filename = format!("{}.tmp", filename);
    let mut file = File::create(&temp_filename).map_err(Error::new_io_error)?;

    let mut segment_stream_headers = Vec::new();

    // delete temp file if errors happen
    let temp_filename_clone = temp_filename.clone();
    defer::defer!({
        // check if the file exists
        if std::fs::metadata(&temp_filename_clone).is_ok() {
            // delete the file
            if std::fs::remove_file(&temp_filename_clone).is_err() {
                log::warn!("Failed to delete temp file: {}", &temp_filename_clone);
            }
        }
    });

    table
        .get_stream_tables()
        .iter()
        .for_each(|(_, stream_table)| {
            let stream_header = SegmentStreamHeader {
                version: SEGMENT_STREAM_HEADER_VERSION_V1,
                crc_check_sum: 0,
                file_offset: 0,
                size: stream_table.size,
                offset: stream_table.offset,
                stream_id: stream_table.stream_id,
            };
            segment_stream_headers.push(stream_header);
        });

    let segment_header = SegmentHeader {
        first_entry: table.get_first_entry(),
        last_entry: table.get_last_entry(),
        version: SEGMENT_HEADER_VERSION_V1,
        stream_headers_offset: SEGMENT_STREAM_HEADER_SIZE,
        stream_headers_count: segment_stream_headers.len() as u64,
    };
    segment_stream_headers.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));

    let mut offset = SEGMENT_HEADER_SIZE as u64;
    // Write the segment stream headers to the file
    file.write_all(unsafe {
        std::slice::from_raw_parts(
            &segment_header as *const SegmentHeader as *const u8,
            SEGMENT_HEADER_SIZE as usize,
        )
    })
    .map_err(Error::new_io_error)?;

    offset += SEGMENT_STREAM_HEADER_SIZE as u64 * segment_stream_headers.len() as u64;

    // Write the segment stream headers to the file
    for stream_header in segment_stream_headers.iter_mut() {
        // update the file offset
        stream_header.file_offset = offset;
        offset += stream_header.size;

        file.write_all(unsafe {
            std::slice::from_raw_parts(
                stream_header as *const SegmentStreamHeader as *const u8,
                SEGMENT_STREAM_HEADER_SIZE as usize,
            )
        })
        .map_err(Error::new_io_error)?;
    }

    // Write the stream data to the file
    for (_, stream_table) in table.get_stream_tables().iter() {
        for stream_data in stream_table.stream_datas.iter() {
            file.write_all(unsafe {
                std::slice::from_raw_parts(
                    stream_data.data.as_ptr() as *const u8,
                    stream_data.size() as usize,
                )
            })
            .map_err(Error::new_io_error)?;
        }
    }

    // flush the file to disk
    file.flush().map_err(Error::new_io_error)?;
    file.sync_all().map_err(Error::new_io_error)?;

    // close the file
    drop(file);

    // rename the file
    std::fs::rename(temp_filename, filename).map_err(Error::new_io_error)?;

    // open file with read only
    let file = File::open(filename).map_err(Error::new_io_error)?;

    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(Error::new_io_error)?;
    let segment = Segment {
        file,
        file_name: filename.to_string(),
        data: mmap,
    };

    // rename the file

    Ok(segment)
}
