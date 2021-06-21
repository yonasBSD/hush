mod flow;
pub mod value;
mod panic;
mod source;
mod lib;
mod mem;

use std::{
	collections::HashMap,
	path::Path,
	ops::Deref,
};

use crate::symbol;
use super::semantic::program;
use value::{
	Array,
	Dict,
	Function,
	HushFun,
	RustFun,
	Value,
};
pub use panic::Panic;
use flow::Flow;
use mem::Stack;
use source::SourcePos;


pub struct Runtime<'a> {
	stack: Stack,
	path: &'static Path,
	interner: &'a mut symbol::Interner,
}


impl<'a> Runtime<'a> {
	pub fn eval(
		program: &'static program::Program,
		interner: &'a mut symbol::Interner
	) -> Result<Value, Panic> {
		let mut runtime = Self {
			stack: Stack::new(),
			path: &program.source,
			interner,
		};

		runtime.stack.extend(program.root_slots.into());

		let std = lib::new();

		runtime.stack.store(mem::SlotIx(0), std);

		match runtime.eval_block(&program.statements)? {
			Flow::Regular(value) => Ok(value),
			flow => panic!("invalid flow in root state: {:#?}", flow)
		}
	}


	fn eval_block(&mut self, block: &'static program::Block) -> Result<Flow, Panic> {
		let mut value = Value::Nil;

		for statement in block.0.iter() {
			match self.eval_statement(statement)? {
				Flow::Regular(val) => value = val,
				Flow::Return(val) => return Ok(Flow::Return(val)),
				Flow::Break => return Ok(Flow::Break)
			}
		}

		Ok(Flow::Regular(value))
	}


	fn eval_literal(
		&mut self,
		literal: &'static program::Literal,
		pos: program::SourcePos
	) -> Result<Flow, Panic> {
		match literal {
			// Nil.
			program::Literal::Nil => Ok(Flow::Regular(Value::Nil)),

			// Bool.
			program::Literal::Bool(b) => Ok(Flow::Regular((*b).into())),

			// Int.
			program::Literal::Int(int) => Ok(Flow::Regular((*int).into())),

			// Float.
			program::Literal::Float(float) => Ok(Flow::Regular((*float).into())),

			// Byte.
			program::Literal::Byte(byte) => Ok(Flow::Regular((*byte).into())),

			// String.
			program::Literal::String(string) => Ok(Flow::Regular(string.as_ref().into())),

			// Array.
			program::Literal::Array(exprs) => {
				let mut array = Vec::new();

				for expr in exprs.iter() {
					match self.eval_expr(expr)? {
						(Flow::Regular(value), _) => array.push(value),
						(flow, _) => return Ok(flow),
					}
				}

				Ok(
					Flow::Regular(Array::new(array).into())
				)
			},

			// Dict.
			program::Literal::Dict(exprs) => {
				let mut dict = HashMap::new();

				for (symbol, expr) in exprs.iter() {
					let key: Value = self.interner
						.resolve(*symbol)
						.expect("unresolved symbol")
						.into();

					match self.eval_expr(expr)? {
						(Flow::Regular(value), _) => dict.insert(key, value),
						(flow, _) => return Ok(flow),
					};
				}

				Ok(
					Flow::Regular(Dict::new(dict).into())
				)
			}

			// Function.
			program::Literal::Function { params, frame_info, body } => {
				let context = frame_info
					.captures
					.iter()
					.map(
						|capture| (
							self.stack.capture(capture.from.into()),
							capture.to.into(),
						)
					)
					.collect();

				Ok(
					Flow::Regular(
						Function::Hush(
							HushFun {
								params: *params,
								frame_info,
								body,
								context,
								pos: SourcePos::new(pos, self.path),
							}
						).into()
					)
				)
			},

			// Identifier.
			program::Literal::Identifier(symbol) => Ok(
				Flow::Regular(
					self.interner
						.resolve(*symbol)
						.expect("unresolved symbol")
						.into()
				)
			),
		}
	}


	fn eval_expr(&mut self, expr: &'static program::Expr) -> Result<(Flow, SourcePos), Panic> {
		match expr {
			// Identifier.
			program::Expr::Identifier { slot_ix, pos } => {
				let value = self.stack.fetch(slot_ix.into());
				self.ok(Flow::Regular(value), *pos)
			},

			// Literal.
			program::Expr::Literal { literal, pos } => {
				let flow = self.eval_literal(literal, *pos)?;
				self.ok(flow, *pos)
			},

			// UnaryOp.
			program::Expr::UnaryOp { op, operand, pos } => {
				use program::UnaryOp::{Minus, Not};

				let (value, operand_pos) = match self.eval_expr(operand)? {
					(Flow::Regular(value), pos) => (value, pos),
					(flow, _) => return self.ok(flow, *pos)
				};

				let value: Value = match (op, value) {
					(Minus, Value::Float(ref f)) => Ok(f.negate().into()),
					(Minus, Value::Int(i)) => Ok((-i).into()),
					(Minus, value) => Err(Panic::invalid_operand(value, operand_pos)),

					(Not, Value::Bool(b)) => Ok((!b).into()),
					(Not, value) => Err(Panic::invalid_operand(value, operand_pos)),
				}?;

				self.ok(Flow::Regular(value), *pos)
			}

			// BinaryOp.
			program::Expr::BinaryOp { left, op, right, pos } => todo!(),

			// If.
			program::Expr::If { condition, then, otherwise, pos } => {
				let condition = match self.eval_expr(condition)? {
					(Flow::Regular(Value::Bool(b)), _) => b,
					(Flow::Regular(value), pos) => return Err(Panic::invalid_condition(value, pos)),
					(flow, _) => return self.ok(flow, *pos)
				};

				let value = if condition {
					self.eval_block(then)
				} else {
					self.eval_block(otherwise)
				}?;

				self.ok(value, *pos)
			}

			// Access.
			program::Expr::Access { object, field, pos } => {
				let (obj, obj_pos) = match self.eval_expr(object)? {
					(Flow::Regular(obj), pos) => (obj, pos),
					(flow, _) => return self.ok(flow, *pos)
				};

				let (field, field_pos) = match self.eval_expr(field)? {
					(Flow::Regular(field), pos) => (field, pos),
					(flow, _) => return self.ok(flow, *pos)
				};

				let value = match (obj, field) {
					(Value::Dict(ref dict), field) => dict
						.get(&field)
						.ok_or(Panic::index_out_of_bounds(field, field_pos)),

					(Value::Array(ref array), Value::Int(ix)) => array
						.index(ix)
						.ok_or(Panic::index_out_of_bounds(Value::Int(ix), field_pos)),

					(Value::Array(_), field) => Err(Panic::invalid_operand(field, field_pos)),

					(obj, _) => Err(Panic::invalid_operand(obj, obj_pos)),
				}?;

				self.ok(Flow::Regular(value), *pos)
			}

			// Call.
			program::Expr::Call { function, args, pos } => {
				let function = match self.eval_expr(function)? {
					(Flow::Regular(Value::Function(ref fun)), _) => fun.clone(),
					(Flow::Regular(value), pos) => return Err(Panic::invalid_call(value, pos)),
					(flow, _) => return self.ok(flow, *pos),
				};

				let mut slots = mem::SlotIx(args.len() as u32);

				self.stack.extend(slots.clone());

				for (ix, expr) in args.iter().enumerate() {
					let slot_ix = mem::SlotIx(ix as u32);

					match self.eval_expr(expr)? {
						(Flow::Regular(value), _) => self.stack.store(slot_ix, value),
						(flow, _) => {
							self.stack.shrink(slots);
							return self.ok(flow, *pos);
						}
					}
				}

				let value = match function.deref() {
					Function::Rust(RustFun { name, fun }) => fun(&mut self.stack, slots.clone())?,

					Function::Hush(HushFun { params, frame_info, body, context, pos }) => {
						if slots.0 != *params {
							return Err(Panic::MissingParameters { pos: pos.clone() });
						}

						let locals: mem::SlotIx = frame_info.slots.into();
						self.stack.extend(locals.clone());
						slots += locals;

						for (value, slot_ix) in context.iter() {
							self.stack.place(slot_ix.clone(), value.clone());
						}
						// TODO: place self parameter.

						let value = match self.eval_block(body)? {
							Flow::Regular(value) => value,
							Flow::Return(value) => value,
							Flow::Break => panic!("break outside loop"),
						};

						value
					}
				};

				self.stack.shrink(slots);

				self.ok(Flow::Regular(value), *pos)
			}

			// CommandBlock.
			program::Expr::CommandBlock { block, pos } => todo!(),
		}
	}


	fn eval_statement(&mut self, statement: &'static program::Statement) -> Result<Flow, Panic> {
		match statement {
			program::Statement::Assign { left, right } => {
				let value = match self.eval_expr(right)?.0 {
					Flow::Regular(value) => value,
					flow => return Ok(flow),
				};

				match left {
					program::Lvalue::Identifier { slot_ix, .. } => self.stack.store(slot_ix.into(), value),
					program::Lvalue::Access { object, field, pos } => {
						let (obj, obj_pos) = match self.eval_expr(object)? {
							(Flow::Regular(obj), pos) => (obj, pos),
							(flow, _) => return Ok(flow),
						};

						let (field, field_pos) = match self.eval_expr(field)? {
							(Flow::Regular(field), pos) => (field, pos),
							(flow, _) => return Ok(flow),
						};

						match (obj, field) {
							(Value::Dict(ref dict), field) => dict.insert(field, value),

							(Value::Array(ref array), Value::Int(ix)) if ix >= array.len() => return Err(
								Panic::index_out_of_bounds(Value::Int(ix), field_pos)
							),

							(Value::Array(ref array), Value::Int(ix)) => array.deref().set(ix, value),

							(Value::Array(_), field) => return Err(Panic::invalid_operand(field, field_pos)),

							(obj, _) => return Err(Panic::invalid_operand(obj, obj_pos)),
						};
					}
				}

				Ok(Flow::Regular(Value::Nil))
			}

			program::Statement::Return { expr } => {
				match self.eval_expr(expr)?.0 {
					Flow::Regular(value) => Ok(Flow::Return(value)),
					flow => Ok(flow),
				}
			}

			program::Statement::Break => Ok(Flow::Break),

			program::Statement::While { condition, block } => {
				loop {
					let condition = match self.eval_expr(condition)? {
						(Flow::Regular(Value::Bool(b)), _) => b,
						(Flow::Regular(value), pos) => return Err(Panic::invalid_condition(value, pos)),
						(flow, _) => return Ok(flow)
					};

					if !condition {
						break;
					}

					match self.eval_block(block)? {
						Flow::Regular(_) => (),
						flow @ Flow::Return(_) => return Ok(flow),
						Flow::Break => break,
					}
				}

				Ok(Flow::Regular(Value::Nil))
			}

			program::Statement::For { slot_ix, expr, block } => todo!(),

			program::Statement::Expr(expr) => self
				.eval_expr(expr)
				.map(|(flow, _)| flow)
		}
	}


	fn ok<T, E>(&self, value: T, pos: program::SourcePos) -> Result<(T, SourcePos), E> {
		Ok((
			value,
			SourcePos::new(pos, self.path)
		))
	}
}
