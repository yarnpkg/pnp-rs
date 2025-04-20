use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::error::Error;
use byteorder::{ReadBytesExt, LittleEndian};
use std::io::Read;

use crate::fs::FileType;

#[derive(Debug)]
pub enum Compression {
    Uncompressed,
    Deflate,
}

#[derive(Debug)]
pub struct Entry {
    pub compression: Compression,
    pub offset: usize,
    pub size: usize,
}

#[derive(Debug)]
pub struct Zip<T> where T : AsRef<[u8]> {
    storage: T,
    pub files: HashMap<String, Entry>,
    pub dirs: HashSet<String>,
}

impl<T> Zip<T>
where T : AsRef<[u8]> {
    pub fn new(storage: T) -> Result<Zip<T>, Box<dyn Error>> {
        let mut zip = Zip {
            storage,
            files: Default::default(),
            dirs: Default::default(),
        };

        for (name, maybe_entry) in list_zip_entries(zip.storage.as_ref())? {
            let name = arca::path::normalize_path(name);
            let segments: Vec<&str> = name.split('/').collect();

            for t in 1..segments.len() - 1 {
                let dir = segments[0..t].to_vec().join("/");
                zip.dirs.insert(dir + "/");
            }

            if let Some(entry) = maybe_entry {
                zip.files.insert(name, entry);
            } else {
                zip.dirs.insert(name);
            }
        }

        Ok(zip)
    }

    pub fn file_type(&self, p: &str) -> Result<FileType, std::io::Error> {
        if self.is_dir(p) {
            Ok(FileType::Directory)
        } else if self.files.contains_key(p) {
            Ok(FileType::File)
        } else {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
    }

    fn is_dir(&self, p: &str) -> bool {
        if p.ends_with('/') {
            self.dirs.contains(p)
        } else {
            self.dirs.contains(&format!("{}/", p))
        }
    }

    pub fn read(&self, p: &str) -> Result<Vec<u8>, std::io::Error> {
        let entry = self.files.get(p)
            .ok_or(std::io::Error::from(std::io::ErrorKind::NotFound))?;

        let data = self.storage.as_ref();
        let slice = &data[entry.offset..entry.offset + entry.size];

        match entry.compression {
            Compression::Deflate => {
                let decompressed_data = miniz_oxide::inflate::decompress_to_vec(&slice)
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "Error during decompression"))?;

                Ok(decompressed_data)
            }

            Compression::Uncompressed => {
                Ok(slice.to_vec())
            }
        }
    }

    pub fn read_to_string(&self, p: &str) -> Result<String, std::io::Error> {
        let data = self.read(p)?;

        Ok(io_bytes_to_str(data.as_slice())?.to_string())
    }
}

fn io_bytes_to_str(vec: &[u8]) -> Result<&str, std::io::Error> {
    std::str::from_utf8(vec)
        .map_err(|_| make_io_utf8_error())
}

fn make_io_utf8_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "File did not contain valid UTF-8"
    )
}

pub fn list_zip_entries(data: &[u8]) -> Result<HashMap<String, Option<Entry>>, Box<dyn Error>> {
    let mut zip_entries = HashMap::new();
    let mut cursor = Cursor::new(data);

    let central_directory_offset = find_central_directory_offset(&mut cursor)?;
    cursor.set_position(central_directory_offset);

    while let Some(entry) = read_central_file_header(&mut cursor)? {
        let entry_name = entry.0;
        let entry_data = entry.1;

        zip_entries.insert(entry_name, entry_data);
    }

    Ok(zip_entries)
}

fn find_central_directory_offset(cursor: &mut Cursor<&[u8]>) -> Result<u64, Box<dyn Error>> {
    cursor.set_position(cursor.get_ref().len() as u64 - 22);
    while cursor.position() > 0 {
        let signature = cursor.read_u32::<LittleEndian>()?;
        if signature == 0x06054b50 {
            cursor.set_position(cursor.position() + 12);
            let central_directory_offset = cursor.read_u32::<LittleEndian>()? as u64;
            return Ok(central_directory_offset);
        }
        cursor.set_position(cursor.position() - 5);
    }
    Err("End of central directory record not found.".into())
}

fn read_central_file_header(cursor: &mut Cursor<&[u8]>) -> Result<Option<(String, Option<Entry>)>, Box<dyn Error>> {
    let signature = cursor.read_u32::<LittleEndian>()?;
    if signature != 0x02014b50 {
        return Ok(None);
    }

    cursor.set_position(cursor.position() + 4); // skip version made by and version needed to extract
    cursor.set_position(cursor.position() + 2); // skip general purpose bit flag

    let compression_method = cursor.read_u16::<LittleEndian>()?;
    cursor.set_position(cursor.position() + 4); // skip last mod time and date

    let compression = match compression_method {
        0 => Ok(Compression::Uncompressed),
        8 => Ok(Compression::Deflate),
        _ => Err("Oh no"),
    }.unwrap();

    let _crc32 = cursor.read_u32::<LittleEndian>()?;
    let compressed_size = cursor.read_u32::<LittleEndian>()? as u64;
    let _uncompressed_size = cursor.read_u32::<LittleEndian>()? as u64;

    let file_name_length = cursor.read_u16::<LittleEndian>()? as usize;
    let extra_field_length = cursor.read_u16::<LittleEndian>()? as usize;
    let comment_length = cursor.read_u16::<LittleEndian>()? as usize;

    let _disk_number_start = cursor.read_u16::<LittleEndian>()?;
    let _internal_file_attributes = cursor.read_u16::<LittleEndian>()?;
    let _external_file_attributes = cursor.read_u32::<LittleEndian>()?;
    let local_header_offset = cursor.read_u32::<LittleEndian>()? as u64;

    let mut file_name_bytes = vec![0; file_name_length];
    cursor.read_exact(&mut file_name_bytes)?;
    let file_name = String::from_utf8(file_name_bytes)?;

    if file_name.ends_with('/') {
        return Ok(Some((file_name, None)));
    }

    cursor.set_position(cursor.position() + extra_field_length as u64 + comment_length as u64);

    let mut local_file_header_cursor = cursor.clone();
    local_file_header_cursor.set_position(local_header_offset + 26);

    let local_file_header_file_name_length = local_file_header_cursor.read_u16::<LittleEndian>()? as usize;
    let local_file_header_extra_field_length = local_file_header_cursor.read_u16::<LittleEndian>()? as usize;
    let file_data_offset = local_header_offset + 30 + local_file_header_file_name_length as u64 + local_file_header_extra_field_length as u64;

    let entry = Entry {
        compression,
        offset: file_data_offset.try_into()?,
        size: compressed_size.try_into()?,
    };

    Ok(Some((file_name, Some(entry))))
}
