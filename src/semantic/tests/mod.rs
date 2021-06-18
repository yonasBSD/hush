use std::{io, path::Path};

use crate::{fmt, symbol, syntax, tests};
use super::{program, Analyzer, Program, Errors};


fn test_dir<P, F>(path: P, mut check: F) -> io::Result<()>
where
	P: AsRef<Path>,
	F: FnMut(&Result<Program, Errors>) -> bool,
{
	let mut interner = symbol::Interner::new();

	tests::util::test_dir(
		path,
		move |path, file| {
			let source = syntax::Source::from_reader(path, file)?;
			let syntactic_analysis = syntax::Analysis::analyze(source, &mut interner);
			let result = Analyzer::analyze(syntactic_analysis.ast, &mut interner);

			if !check(&result) {
				match result {
					Ok(program) => panic!(
						"{}",
						fmt::Show(
							program,
							program::fmt::Context::from(&interner),
						)
					),

					Err(errors) => panic!("{}", fmt::Show(errors, &interner)),
				}
			}

			Ok(())
		}
	)
}


#[test]
fn test_examples() -> io::Result<()> {
	test_dir(
		"examples/hush",
		Result::is_ok,
	)
}


// #[test]
// fn test_positive() -> io::Result<()> {
// 	test_dir(
// 		"src/syntax/tests/data/positive",
// 		|analysis| analysis.errors.is_empty(),
// 	)
// }


#[test]
fn test_negative() -> io::Result<()> {
	test_dir(
		"src/semantic/tests/data/negative",
		Result::is_err,
	)
}