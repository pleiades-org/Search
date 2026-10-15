use std::{
    hint::black_box,
    io::{self, Write},
    time::Instant,
};

pub struct Measurements<'output> {
    pub summary: &'output mut dyn Write,
    pub samples: &'output mut dyn Write,
}

impl Measurements<'_> {
    pub fn measure(
        &mut self,
        case: &str,
        sample_count: usize,
        expected_count: usize,
        mut operation: impl FnMut() -> usize,
    ) -> io::Result<()> {
        for _ in 0..8 {
            black_box(operation());
        }
        let mut durations = Vec::with_capacity(sample_count);
        for sample in 0..sample_count {
            let start = Instant::now();
            let count = black_box(operation());
            let elapsed_ns = start.elapsed().as_nanos();
            assert_eq!(count, expected_count, "incorrect result count for {case}");
            durations.push(elapsed_ns);
            writeln!(self.samples, "{case},{sample},{elapsed_ns},{count}")?;
        }
        durations.sort_unstable();
        let median = durations[sample_count / 2] as f64 / 1000.0;
        let p95 = durations[(sample_count * 95).div_ceil(100) - 1] as f64 / 1000.0;
        writeln!(
            self.summary,
            "{case},{sample_count},{expected_count},{median:.3},{p95:.3}"
        )
    }
}
