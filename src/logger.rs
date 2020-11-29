use colorful::{core::color_string::CString, Color, Colorful};
use log::{kv, LevelFilter, Log, Metadata, Record};
use std::io::{self, StdoutLock, Write};

/// Start logging.
//pub(crate) fn start(level: LevelFilter) {
pub(crate) fn start() {
    let logger = Box::new(Logger {});
    log::set_boxed_logger(logger).expect("Could not start logging");
    log::set_max_level(LevelFilter::Trace);
}

#[derive(Debug)]
pub(crate) struct Logger {}

impl Log for Logger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        // metadata.level() <= log::max_level()
        true
    }

    fn log(&self, record: &Record<'_>) {
        use chrono::prelude::*;

        if record
            .module_path()
            .map(|m| m.contains("rustorrent"))
            .unwrap_or(true)
        {
            // if self.enabled(record.metadata()) {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            let level = get_level(record.level());
            let time = Local::now().format("%T");
            //let time = time::UNIX_EPOCH.elapsed().unwrap().as_millis();
            write!(&mut handle, "{} {}: ", time, level).unwrap();
            // write!(&mut handle, "{{\"level\":{},\"time\":{},\"msg\":", level, time).unwrap();
            write!(&mut handle, "{}", record.args()).unwrap();
            // serde_json::to_writer(&mut handle, record.args()).unwrap();
            format_kv_pairs(&mut handle, &record);
            // writeln!(&mut handle, " }}").unwrap();
            writeln!(&mut handle).unwrap();
        }
    }

    // fn log(&self, record: &Record<'_>) {
    //     if self.enabled(record.metadata()) {
    //         let stdout = io::stdout();
    //         let mut handle = stdout.lock();
    //         let level = get_level(record.level());
    //         let time = time::UNIX_EPOCH.elapsed().unwrap().as_millis();
    //         write!(&mut handle, "{{\"level\":{},\"time\":{},\"msg\":", level, time).unwrap();
    //         serde_json::to_writer(&mut handle, record.args()).unwrap();
    //         format_kv_pairs(&mut handle, &record);
    //         writeln!(&mut handle, "}}").unwrap();
    //     }
    // }
    fn flush(&self) {}
}

fn get_level(level: log::Level) -> CString {
    use log::Level::*;

    match level {
        Trace => "TRACE".color(Color::Magenta),
        Debug => "DEBUG".color(Color::Blue),
        Info => "INFO".color(Color::Green),
        Warn => "WARN".color(Color::Yellow),
        Error => "ERROR".color(Color::Red),
    }
}

fn format_kv_pairs<'b>(mut out: &mut StdoutLock<'b>, record: &Record) {
    struct Visitor<'a, 'b> {
        string: &'a mut StdoutLock<'b>,
    }

    impl<'kvs, 'a, 'b> kv::Visitor<'kvs> for Visitor<'a, 'b> {
        fn visit_pair(
            &mut self,
            key: kv::Key<'kvs>,
            val: kv::Value<'kvs>,
        ) -> Result<(), kv::Error> {
            write!(
                self.string,
                " {}: {}",
                key.to_string().color(Color::Yellow).bold(),
                val
            )
            .unwrap();
            // write!(self.string, ",\"{}\":{}", key, val).unwrap();
            Ok(())
        }
    }

    let key_values = record.key_values();

    if key_values.count() > 0 {
        write!(out, " {{").unwrap();
        let mut visitor = Visitor { string: &mut out };
        record.key_values().visit(&mut visitor).unwrap();
        write!(out, " }}").unwrap();
    }
}
