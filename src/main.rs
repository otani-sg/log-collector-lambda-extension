use aws_sdk_s3 as s3;
use chrono::Utc;
use enum_as_inner::EnumAsInner;
use lambda_extension::{service_fn, Error, Extension, LambdaEvent, NextEvent};
use log::{debug, error, info, warn, log_enabled, Level};
use parquet::basic::{Compression, Repetition};
use parquet::column::writer::ColumnWriter;
use parquet::data_type::ByteArray;
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::printer::print_schema;
use s3::types::{ByteStream, DisplayErrorContext};
use serde::{Deserialize, Serialize};
use serde_json::to_string;
use std::collections::HashMap;
use std::env;
use tokio::sync::broadcast::{self, Sender};
use warp::hyper::StatusCode;
// use std::fs::File;
// use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::signal;
use uuid::Uuid;
use warp::Filter;

pub struct Config {
    pub port: u16,
    pub s3_bucket: String,
    pub s3_bucket_prefix: String,
}

impl Config {
    pub fn build() -> Result<Config, &'static str> {
        let port = env::var("LCLE_PORT")
            .unwrap_or("3030".to_owned())
            .parse::<u16>()
            .expect("LCLE_PORT environment variable is invalid");

        let s3_bucket = match env::var("LCLE_S3_BUCKET") {
            Ok(s3_bucket) => s3_bucket,
            Err(_) => {
                warn!("LCLE_S3_BUCKET is not set. The extension will only collect logs but don't know where to store them.");
                "".to_owned()
            }
        };
        let s3_bucket_prefix =
            env::var("LCLE_S3_BUCKET_PREFIX").unwrap_or("".to_owned());

        Ok(Config {
            port,
            s3_bucket,
            s3_bucket_prefix,
        })
    }
}

#[derive(Serialize, Deserialize, EnumAsInner, Debug)]
#[serde(untagged)]
enum PrimaryValue {
    String(String),
    Integer(i64),
    Double(f64),
    Bool(bool),
}

type LogEntry = HashMap<String, PrimaryValue>;

type LogEntries = Arc<Mutex<Vec<LogEntry>>>;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(
        Env::default().default_filter_or("log_collector_lambda_extension=info"),
    )
    .init();

    let config = Config::build().unwrap();

    let entries: LogEntries = Arc::new(Mutex::new(Vec::new()));
    let g_entries = entries.clone();

    // POST / Receive json body
    let endpoint = warp::path("collect")
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(warp::any().map(move || entries.clone()))
        .map(move |mut body: LogEntry, entries: LogEntries| {
            // Add timestamp to every entry
            let ts = Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
            body.insert("_timestamp".to_owned(), PrimaryValue::String(ts));

            // let response = format!("Got a JSON body: {}!", to_string(&body).unwrap());
            entries.lock().unwrap().push(body);
            // return response;
            StatusCode::ACCEPTED
        });

    // One time channel to trigger server shutdown when receiving lambda shutdown event
    let (lambda_shutdown_tx, mut lambda_shutdown_rx) = broadcast::channel::<String>(1);

    let (_addr, fut) = warp::serve(endpoint).bind_with_graceful_shutdown(
        ([127, 0, 0, 1], config.port),
        async move {
            // Start shutdown procedure when either we receive SHUTDOWN event
            // from AWS Lambda, or SIGTERM signal when testing locally
            tokio::select! {
                val = lambda_shutdown_rx.recv() => {
                    println!("Caught shutdown event from Lambda Runtime: {:?}", val);
                },
                _ = signal::ctrl_c() => {
                    println!("Caught Ctrl+C");
                }
            };

            flush_to_files(config, g_entries).await
        },
    );

    tokio::spawn(register_lambda_shutdown_event(lambda_shutdown_tx.clone()));

    fut.await
}

async fn flush_to_files(config: Config, entries: LogEntries) {
    let parquets = to_parquets(entries);

    // Intialize S3 client if LCLE_S3_BUCKET is set
    let mut client: Option<s3::Client> = None;
    if config.s3_bucket.len() > 0 {
        let aws_config = aws_config::load_from_env().await;
        client = Some(s3::Client::new(&aws_config));
    }
    let mut full_path;

    // Upload each file to S3
    for (s3_path, parquet) in parquets {
        full_path = format!("{}/{}", config.s3_bucket_prefix, s3_path);
        full_path = match full_path.strip_prefix("/") {
            Some(stripped) => stripped.to_string(),
            None => full_path,
        };

        // Write to file for debugging
        // let out_path = "./test.parquet";
        // let mut out_file = File::create(&out_path).unwrap();
        // out_file.write_all(&buffer).unwrap();

        match client {
            Some(ref client) => {
                match client
                    .put_object()
                    .bucket(config.s3_bucket.to_owned())
                    .body(ByteStream::from(parquet))
                    .key(full_path.to_string())
                    .send()
                    .await
                {
                    Ok(_) => info!(
                        "Logs written succesfully to s3://{}/{}",
                        config.s3_bucket, full_path
                    ),
                    Err(err) => error!(
                        "Could not write to {}: {}",
                        full_path,
                        DisplayErrorContext(err)
                    ),
                };
            }
            None => {
                debug!(
                    "Logs would have been written to s3://<LCLE_S3_BUCKET>/{}",
                    full_path
                )
            }
        }
    }
}

async fn register_lambda_shutdown_event(lambda_shutdown_tx: Sender<String>) {
    // Only run this block on lambda environment
    match env::var("AWS_LAMBDA_RUNTIME_API") {
        Ok(_) => {}
        Err(_) => return,
    }
    let guarded_tx = Arc::new(Mutex::new(lambda_shutdown_tx));

    // A handler that simply send shutdown signal
    let events_processor = service_fn(|request: LambdaEvent| {
        let cloned_tx = guarded_tx.clone();
        async move {
            match request.next {
                NextEvent::Shutdown(event) => {
                    cloned_tx
                        .clone()
                        .lock()
                        .unwrap()
                        .send(event.shutdown_reason)
                        .unwrap();
                }
                NextEvent::Invoke(_) => {}
            }
            Ok::<(), Error>(())
        }
    });

    // Register for the shutdown event with above handler
    let extension = Extension::new()
        .with_events(&["SHUTDOWN"])
        .with_events_processor(events_processor);

    extension.run().await.unwrap()
}

/// Write log entries into potentially multiple parquet buffers, where the key
/// for each buffer is where to write it on S3
fn to_parquets(entries: LogEntries) -> HashMap<String, Vec<u8>> {
    let _entries = &*entries.lock().unwrap();
    if _entries.len() == 0 {
        return HashMap::new();
    };

    // Debug entries
    if log_enabled!(Level::Debug) {
        debug!(
            "Number of entries: {}, first 5: {}",
            _entries.len(),
            to_string(&_entries.iter().take(5).collect::<Vec<&LogEntry>>()).unwrap()
        );
    }

    // Build schema from the first entry
    let schema = build_schema(&_entries[0]);

    // Debug schema
    if log_enabled!(Level::Debug) {
        let mut schema_buffer = Vec::new();
        print_schema(&mut schema_buffer, &schema);
        debug!("Detected schema: {}", String::from_utf8(schema_buffer).unwrap());
    }

    let entry_groups = group_by_timestamp(_entries);

    let mut parquet_groups = HashMap::new();

    for (group_key, group_entries) in entry_groups {
        // Write entries into parquet-formatted buffer
        let buffer = to_parquet(&schema, &group_entries);

        let file_id = Uuid::new_v4().to_string();
        parquet_groups.insert(
            format!("{}/{}.parquet", group_key, file_id).to_owned(),
            buffer,
        );
    }

    parquet_groups
}

/// Infer parquet schema from keys of a log entry
fn build_schema(entry: &LogEntry) -> parquet::schema::types::Type {
    let mut fields = Vec::new();

    for (key, value) in entry {
        let physical_type = match value {
            PrimaryValue::String(_) => parquet::basic::Type::BYTE_ARRAY,
            PrimaryValue::Integer(_) => parquet::basic::Type::INT64,
            PrimaryValue::Double(_) => parquet::basic::Type::DOUBLE,
            PrimaryValue::Bool(_) => parquet::basic::Type::BOOLEAN,
        };
        let mut field = parquet::schema::types::Type::primitive_type_builder(key, physical_type)
            .with_repetition(Repetition::REQUIRED);

        if matches!(value, PrimaryValue::String(_)) {
            field = field.with_converted_type(parquet::basic::ConvertedType::UTF8)
        }

        fields.push(Arc::new(field.build().unwrap()));
    }

    parquet::schema::types::Type::group_type_builder("log_entry")
        .with_fields(&mut fields)
        .build()
        .unwrap()
}

/// Write list of log entries, using defined schema into parquet-formatted buffer
fn to_parquet(schema: &parquet::schema::types::Type, entries: &Vec<&LogEntry>) -> Vec<u8> {
    let mut buffer = Vec::<u8>::new();

    let props = Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::GZIP)
            .build(),
    );
    let mut writer =
        SerializedFileWriter::new(&mut buffer, Arc::new(schema.clone()), props).unwrap();
    for entry in entries {
        let mut row_group_writer = writer.next_row_group().unwrap();
        for field in schema.get_fields() {
            if let Some(mut col_writer) = row_group_writer.next_column().unwrap() {
                if let Some(value) = entry.get(field.name()) {
                    match col_writer.untyped() {
                        ColumnWriter::BoolColumnWriter(ref mut typed_writer) => match value {
                            PrimaryValue::Bool(_) => typed_writer.write_batch(
                                &[*value.as_bool().unwrap_or(&false)],
                                None,
                                None,
                            ),
                            _ => unimplemented!(),
                        },
                        ColumnWriter::Int64ColumnWriter(ref mut typed_writer) => match value {
                            PrimaryValue::Integer(_) => typed_writer.write_batch(
                                &[*value.as_integer().unwrap_or(&0)],
                                None,
                                None,
                            ),
                            _ => unimplemented!(),
                        },
                        ColumnWriter::DoubleColumnWriter(ref mut typed_writer) => match value {
                            PrimaryValue::Double(_) => typed_writer.write_batch(
                                &[*value.as_double().unwrap_or(&0.0)],
                                None,
                                None,
                            ),
                            _ => unimplemented!(),
                        },
                        ColumnWriter::ByteArrayColumnWriter(ref mut typed_writer) => match value {
                            PrimaryValue::String(_) => typed_writer.write_batch(
                                &[ByteArray::from(
                                    (*value.as_string().unwrap_or(&"".to_owned()))
                                        .as_bytes()
                                        .to_vec(),
                                )],
                                None,
                                None,
                            ),
                            _ => unimplemented!(),
                        },
                        _ => unimplemented!(),
                    }
                    .unwrap();
                }
                col_writer.close().unwrap();
            }
        }
        row_group_writer.close().unwrap();
    }

    writer.close().unwrap();

    buffer
}

fn group_by_timestamp(entries: &Vec<LogEntry>) -> HashMap<String, Vec<&LogEntry>> {
    let mut entry_groups: HashMap<String, Vec<&LogEntry>> = HashMap::new();

    for entry in entries {
        // Using the event's timestamp, e.g 2022-01-01T00:00:00.000000Z,
        // we'll group it under a key composing of date & hour, e.g date=2022-01-01/hour=00
        let timestamp = match entry.get("_timestamp").unwrap() {
            PrimaryValue::String(ref ts) => ts,
            _ => unimplemented!(),
        };
        let mut split = timestamp.split("T");
        let date = split.next().unwrap();
        let hour = &split.next().unwrap()[..2];
        // The format of the key allow automatic partitioning for amazon athena
        let group_key = format!("date={}/hour={}", date, hour);

        match entry_groups.get_mut(&group_key) {
            Some(group) => {
                group.push(entry);
            }
            None => {
                let mut group = Vec::new();
                group.push(entry);
                entry_groups.insert(group_key, group);
            }
        }
    }

    entry_groups
}
