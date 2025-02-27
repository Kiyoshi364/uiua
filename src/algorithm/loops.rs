//! Algorithms for looping modifiers

use std::{
    iter::once,
    ops::{Add, Div, Mul, Sub},
};

use tinyvec::tiny_vec;

use crate::{
    algorithm::pervade::bin_pervade_generic,
    array::{Array, ArrayValue, FormatShape, Shape},
    cowslice::cowslice,
    primitive::Primitive,
    run::{ArrayArg, FunctionArg},
    value::Value,
    Uiua, UiuaResult,
};

fn flip<A, B, C>(f: impl Fn(A, B) -> C) -> impl Fn(B, A) -> C {
    move |b, a| f(a, b)
}

pub fn reduce(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let xs = env.pop(ArrayArg(1))?;

    match (f.as_flipped_primitive(), xs) {
        (Some((prim, flipped)), Value::Num(nums)) => env.push(match prim {
            Primitive::Add => fast_reduce(nums, 0.0, Add::add),
            Primitive::Sub if flipped => fast_reduce(nums, 0.0, Sub::sub),
            Primitive::Sub => fast_reduce(nums, 0.0, flip(Sub::sub)),
            Primitive::Mul => fast_reduce(nums, 1.0, Mul::mul),
            Primitive::Div if flipped => fast_reduce(nums, 1.0, Div::div),
            Primitive::Div => fast_reduce(nums, 1.0, flip(Div::div)),
            Primitive::Max => fast_reduce(nums, f64::NEG_INFINITY, f64::max),
            Primitive::Min => fast_reduce(nums, f64::INFINITY, f64::min),
            _ => return generic_fold(f, Value::Num(nums), None, env),
        }),
        (Some((prim, flipped)), Value::Byte(bytes)) => env.push(match prim {
            Primitive::Add => fast_reduce(bytes, 0.0, |a, b| a + f64::from(b)),
            Primitive::Sub if flipped => fast_reduce(bytes, 0.0, |a, b| a - f64::from(b)),
            Primitive::Sub => fast_reduce(bytes, 0.0, |a, b| f64::from(b) - a),
            Primitive::Mul => fast_reduce(bytes, 1.0, |a, b| a * f64::from(b)),
            Primitive::Div if flipped => fast_reduce(bytes, 1.0, |a, b| a / f64::from(b)),
            Primitive::Div => fast_reduce(bytes, 1.0, |a, b| f64::from(b) / a),
            Primitive::Max => fast_reduce(bytes, f64::NEG_INFINITY, |a, b| a.max(f64::from(b))),
            Primitive::Min => fast_reduce(bytes, f64::INFINITY, |a, b| a.min(f64::from(b))),
            _ => return generic_fold(f, Value::Byte(bytes), None, env),
        }),
        (_, xs) => generic_fold(f, xs, None, env)?,
    }
    Ok(())
}

pub fn fast_reduce<T: ArrayValue + Into<R>, R: ArrayValue>(
    mut arr: Array<T>,
    identity: R,
    f: impl Fn(R, T) -> R,
) -> Array<R> {
    match arr.shape.len() {
        0 => Array::new(
            tiny_vec![],
            vec![arr.data.into_iter().next().unwrap().into()],
        ),
        1 => {
            let mut vals = arr.data.into_iter();
            Array::new(
                tiny_vec![],
                vec![if let Some(acc) = vals.next() {
                    vals.fold(acc.into(), f)
                } else {
                    identity
                }],
            )
        }
        _ => {
            let row_len = arr.row_len();
            let row_count = arr.row_count();
            if row_count == 0 {
                arr.shape.remove(0);
                let data = cowslice![identity; row_len];
                return Array::new(arr.shape, data);
            }
            let mut new_data: Vec<R> = arr.data[..row_len]
                .iter()
                .cloned()
                .map(Into::into)
                .collect();
            for i in 1..row_count {
                let start = i * row_len;
                for j in 0..row_len {
                    new_data[j] = f(new_data[j].clone(), arr.data[start + j].clone());
                }
            }
            arr.shape.remove(0);
            Array::new(arr.shape, new_data)
        }
    }
}

fn generic_fold(f: Value, xs: Value, init: Option<Value>, env: &mut Uiua) -> UiuaResult {
    match f.signature().args {
        0 | 1 => {
            let mut rows = init.into_iter().chain(xs.into_rows());
            while let Some(row) = rows.next() {
                env.push(row);
                if env.call_catch_break(f.clone())? {
                    let reduced = if f.signature().args == 0 {
                        None
                    } else {
                        Some(env.pop("reduced function result")?)
                    };
                    let val = Value::from_row_values(reduced.into_iter().chain(rows), env)?;
                    env.push(val);
                    return Ok(());
                }
            }
        }
        2 => {
            let mut rows = xs.into_rows();
            let mut acc = init
                .or_else(|| rows.next())
                .ok_or_else(|| env.error("Cannot reduce empty array"))?;
            while let Some(row) = rows.next() {
                env.push(row);
                env.push(acc);
                let should_break = env.call_catch_break(f.clone())?;
                acc = env.pop("reduced function result")?;
                if should_break {
                    acc = Value::from_row_values(once(acc).chain(rows), env)?;
                    break;
                }
            }
            env.push(acc);
        }
        args => {
            return Err(env.error(format!(
                "Cannot reduce a function that takes {args} arguments"
            )))
        }
    }
    Ok(())
}

pub fn fold(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let acc = env.pop(ArrayArg(1))?;
    let xs = env.pop(ArrayArg(2))?;
    generic_fold(f, xs, Some(acc), env)
}

pub fn scan(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let xs = env.pop(ArrayArg(1))?;
    if xs.rank() == 0 {
        return Err(env.error("Cannot scan rank 0 array"));
    }
    match (f.as_flipped_primitive(), xs) {
        (Some((prim, flipped)), Value::Num(nums)) => {
            let arr = match prim {
                Primitive::Add => fast_scan(nums, Add::add),
                Primitive::Sub if flipped => fast_scan(nums, Sub::sub),
                Primitive::Sub => fast_scan(nums, flip(Sub::sub)),
                Primitive::Mul => fast_scan(nums, Mul::mul),
                Primitive::Div if flipped => fast_scan(nums, Div::div),
                Primitive::Div => fast_scan(nums, flip(Div::div)),
                Primitive::Max => fast_scan(nums, f64::max),
                Primitive::Min => fast_scan(nums, f64::min),
                _ => return generic_scan(f, Value::Num(nums), env),
            };
            env.push(arr);
            Ok(())
        }
        (Some((prim, flipped)), Value::Byte(bytes)) => {
            match prim {
                Primitive::Add => env.push(fast_scan::<f64>(bytes.convert(), Add::add)),
                Primitive::Sub if flipped => env.push(fast_scan::<f64>(bytes.convert(), Sub::sub)),
                Primitive::Sub => env.push(fast_scan::<f64>(bytes.convert(), flip(Sub::sub))),
                Primitive::Mul => env.push(fast_scan::<f64>(bytes.convert(), Mul::mul)),
                Primitive::Div if flipped => env.push(fast_scan::<f64>(bytes.convert(), Div::div)),
                Primitive::Div => env.push(fast_scan::<f64>(bytes.convert(), flip(Div::div))),
                Primitive::Max => env.push(fast_scan(bytes, u8::max)),
                Primitive::Min => env.push(fast_scan(bytes, u8::min)),
                _ => return generic_scan(f, Value::Byte(bytes), env),
            }
            Ok(())
        }
        (_, xs) => generic_scan(f, xs, env),
    }
}

fn fast_scan<T: ArrayValue>(mut arr: Array<T>, f: impl Fn(T, T) -> T) -> Array<T> {
    match arr.shape.len() {
        0 => unreachable!("fast_scan called on unit array, should have been guarded against"),
        1 => {
            if arr.row_count() == 0 {
                return arr;
            }
            let mut acc = arr.data[0].clone();
            for val in arr.data.iter_mut().skip(1) {
                acc = f(acc, val.clone());
                *val = acc.clone();
            }
            arr
        }
        _ => {
            let row_len: usize = arr.row_len();
            if arr.row_count() == 0 {
                return arr;
            }
            let shape = arr.shape.clone();
            let mut new_data = Vec::with_capacity(arr.data.len());
            let mut rows = arr.into_rows();
            new_data.extend(rows.next().unwrap().data);
            for row in rows {
                let start = new_data.len() - row_len;
                for (i, r) in row.data.into_iter().enumerate() {
                    new_data.push(f(new_data[start + i].clone(), r));
                }
            }
            Array::new(shape, new_data)
        }
    }
}

fn generic_scan(f: Value, xs: Value, env: &mut Uiua) -> UiuaResult {
    if xs.row_count() == 0 {
        env.push(xs.first_dim_zero());
        return Ok(());
    }
    let row_count = xs.row_count();
    let mut rows = xs.into_rows();
    let mut acc = rows.next().unwrap();
    let mut scanned = Vec::with_capacity(row_count);
    scanned.push(acc.clone());
    for row in rows {
        let start_height = env.stack_size();
        env.push(row);
        env.push(acc.clone());
        let should_break = env.call_catch_break(f.clone())?;
        acc = env.pop("scanned function result")?;
        scanned.push(acc.clone());
        if should_break {
            env.truncate_stack(start_height);
            break;
        }
    }
    env.push(Value::from_row_values(scanned, env)?);
    Ok(())
}

pub fn each(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let sig = f.signature();
    let output = match sig.outputs {
        0 => false,
        1 => true,
        n => {
            return Err(env.error(format!(
                "Each's function must return 0 or 1 values, but it returns {}",
                n
            )))
        }
    };
    match sig.args {
        0 => Ok(()),
        1 => {
            let xs = env.pop(ArrayArg(1))?;
            if output {
                each1_1(f, xs, env)
            } else {
                each1_0(f, xs, env)
            }
        }
        2 => {
            let xs = env.pop(ArrayArg(1))?;
            let ys = env.pop(ArrayArg(2))?;
            if output {
                each2_1(f, xs, ys, env)
            } else {
                each2_0(f, xs, ys, env)
            }
        }
        n => {
            let mut args = Vec::with_capacity(n);
            for i in 0..n {
                args.push(env.pop(ArrayArg(i + 1))?);
            }
            if output {
                eachn_1(f, args, env)
            } else {
                eachn_0(f, args, env)
            }
        }
    }
}

fn each1_1(f: Value, xs: Value, env: &mut Uiua) -> UiuaResult {
    let mut new_values = Vec::with_capacity(xs.flat_len());
    let mut new_shape = Shape::from(xs.shape());
    let mut old_values = xs.into_flat_values();
    for val in old_values.by_ref() {
        env.push(val);
        let broke = env.call_catch_break(f.clone())?;
        new_values.push(env.pop("each's function result")?);
        if broke {
            for row in old_values {
                new_values.push(row);
            }
            break;
        }
    }
    let mut eached = Value::from_row_values(new_values, env)?;
    new_shape.extend_from_slice(&eached.shape()[1..]);
    *eached.shape_mut() = new_shape;
    env.push(eached);
    Ok(())
}

fn each1_0(f: Value, xs: Value, env: &mut Uiua) -> UiuaResult {
    let values = xs.into_flat_values();
    for val in values {
        env.push(val);
        if env.call_catch_break(f.clone())? {
            break;
        }
    }
    Ok(())
}

fn each2_1(f: Value, xs: Value, ys: Value, env: &mut Uiua) -> UiuaResult {
    let xs_shape = xs.shape().to_vec();
    let ys_shape = ys.shape().to_vec();
    let xs_values: Vec<_> = xs.into_flat_values().collect();
    let ys_values: Vec<_> = ys.into_flat_values().collect();
    let (mut shape, values) = bin_pervade_generic(
        &xs_shape,
        xs_values,
        &ys_shape,
        ys_values,
        env,
        |x, y, env| {
            env.push(y);
            env.push(x);
            env.call_error_on_break(f.clone(), "break is not allowed in multi-argument each")?;
            env.pop("each's function result")
        },
    )?;
    let mut eached = Value::from_row_values(values, env)?;
    shape.extend_from_slice(&eached.shape()[1..]);
    *eached.shape_mut() = shape;
    env.push(eached);
    Ok(())
}

fn each2_0(f: Value, xs: Value, ys: Value, env: &mut Uiua) -> UiuaResult {
    let xs_shape = xs.shape().to_vec();
    let ys_shape = ys.shape().to_vec();
    let xs_values: Vec<_> = xs.into_flat_values().collect();
    let ys_values: Vec<_> = ys.into_flat_values().collect();
    bin_pervade_generic(
        &xs_shape,
        xs_values,
        &ys_shape,
        ys_values,
        env,
        |x, y, env| {
            env.push(y);
            env.push(x);
            env.call_error_on_break(f.clone(), "break is not allowed multi-argument in each")?;
            Ok(())
        },
    )?;
    Ok(())
}

fn eachn_1(f: Value, args: Vec<Value>, env: &mut Uiua) -> UiuaResult {
    for win in args.windows(2) {
        if win[0].shape() != win[1].shape() {
            return Err(env.error(format!(
                "The shapes in each of 3 or more arrays must all match, but shapes {} and {} cannot be eached together. \
                If you want more flexibility, use rows.",
                win[0].format_shape(),
                win[1].format_shape()
            )));
        }
    }
    let elem_count = args[0].flat_len();
    let mut arg_elems: Vec<_> = args.into_iter().map(|v| v.into_flat_values()).collect();
    let mut new_values = Vec::new();
    for _ in 0..elem_count {
        for arg in arg_elems.iter_mut().rev() {
            env.push(arg.next().unwrap());
        }
        env.call_error_on_break(f.clone(), "break is not allowed in multi-argument each")?;
        new_values.push(env.pop("each's function result")?);
    }
    let eached = Value::from_row_values(new_values, env)?;
    env.push(eached);
    Ok(())
}

fn eachn_0(f: Value, args: Vec<Value>, env: &mut Uiua) -> UiuaResult {
    for win in args.windows(2) {
        if win[0].shape() != win[1].shape() {
            return Err(env.error(format!(
                "The shapes in each of 3 or more arrays must all match, but shapes {} and {} cannot be eached together. \
                If you want more flexibility, use rows.",
                win[0].format_shape(),
                win[1].format_shape()
            )));
        }
    }
    let elem_count = args[0].flat_len();
    let mut arg_elems: Vec<_> = args.into_iter().map(|v| v.into_flat_values()).collect();
    for _ in 0..elem_count {
        for arg in arg_elems.iter_mut().rev() {
            env.push(arg.next().unwrap());
        }
        env.call_error_on_break(f.clone(), "break is not allowed in multi-argument each")?;
    }
    Ok(())
}

pub fn rows(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let sig = f.signature();
    let output = match sig.outputs {
        0 => false,
        1 => true,
        n => {
            return Err(env.error(format!(
                "Rows's function must return 0 or 1 values, but it returns {}",
                n
            )))
        }
    };
    match sig.args {
        0 => Ok(()),
        1 => {
            let xs = env.pop(ArrayArg(1))?;
            if output {
                rows1_1(f, xs, env)
            } else {
                rows1_0(f, xs, env)
            }
        }
        2 => {
            let xs = env.pop(ArrayArg(1))?;
            let ys = env.pop(ArrayArg(2))?;
            if output {
                rows2_1(f, xs, ys, env)
            } else {
                rows2_0(f, xs, ys, env)
            }
        }
        n => {
            let mut args = Vec::with_capacity(n);
            for i in 0..n {
                args.push(env.pop(ArrayArg(i + 1))?);
            }
            if output {
                rowsn_1(f, args, env)
            } else {
                rowsn_0(f, args, env)
            }
        }
    }
}

fn rows1_1(f: Value, xs: Value, env: &mut Uiua) -> UiuaResult {
    let mut new_rows = Vec::with_capacity(xs.row_count());
    let mut old_rows = xs.into_rows();
    for row in old_rows.by_ref() {
        env.push(row);
        let broke = env.call_catch_break(f.clone())?;
        new_rows.push(env.pop("rows' function result")?);
        if broke {
            for row in old_rows {
                new_rows.push(row);
            }
            break;
        }
    }
    let res = Value::from_row_values(new_rows, env)?;
    env.push(res);
    Ok(())
}

fn rows1_0(f: Value, xs: Value, env: &mut Uiua) -> UiuaResult {
    for row in xs.into_rows() {
        env.push(row);
        let broke = env.call_catch_break(f.clone())?;
        if broke {
            break;
        }
    }
    Ok(())
}

fn rows2_1(f: Value, xs: Value, ys: Value, env: &mut Uiua) -> UiuaResult {
    if xs.row_count() != ys.row_count() {
        return Err(env.error(format!(
            "Cannot rows arrays with different number of rows {} and {}",
            xs.row_count(),
            ys.row_count()
        )));
    }
    let mut new_rows = Vec::with_capacity(xs.row_count());
    let x_rows = xs.into_rows();
    let y_rows = ys.into_rows();
    for (x, y) in x_rows.into_iter().zip(y_rows) {
        env.push(y);
        env.push(x);
        env.call_error_on_break(f.clone(), "break is not allowed in multi-argument rows")?;
        new_rows.push(env.pop("rows's function result")?);
    }
    env.push(Value::from_row_values(new_rows, env)?);
    Ok(())
}

fn rows2_0(f: Value, xs: Value, ys: Value, env: &mut Uiua) -> UiuaResult {
    if xs.row_count() != ys.row_count() {
        return Err(env.error(format!(
            "Cannot rows arrays with different number of rows {} and {}",
            xs.row_count(),
            ys.row_count()
        )));
    }
    let x_rows = xs.into_rows();
    let y_rows = ys.into_rows();
    for (x, y) in x_rows.into_iter().zip(y_rows) {
        env.push(y);
        env.push(x);
        env.call_error_on_break(f.clone(), "break is not allowed in multi-argument rows")?;
    }
    Ok(())
}

fn rowsn_1(f: Value, args: Vec<Value>, env: &mut Uiua) -> UiuaResult {
    let row_count = args[0].row_count();
    let mut arg_elems: Vec<_> = args.into_iter().map(|v| v.into_rows()).collect();
    let mut new_values = Vec::new();
    for _ in 0..row_count {
        for arg in arg_elems.iter_mut().rev() {
            env.push(arg.next().unwrap());
        }
        env.call_error_on_break(f.clone(), "break is not allowed in multi-argument each")?;
        new_values.push(env.pop("each's function result")?);
    }
    let eached = Value::from_row_values(new_values, env)?;
    env.push(eached);
    Ok(())
}

fn rowsn_0(f: Value, args: Vec<Value>, env: &mut Uiua) -> UiuaResult {
    let row_count = args[0].row_count();
    let mut arg_elems: Vec<_> = args.into_iter().map(|v| v.into_rows()).collect();
    for _ in 0..row_count {
        for arg in arg_elems.iter_mut().rev() {
            env.push(arg.next().unwrap());
        }
        env.call_error_on_break(f.clone(), "break is not allowed in multi-argument each")?;
    }
    Ok(())
}

pub fn distribute(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let xs = env.pop(ArrayArg(1))?;
    let y = env.pop(ArrayArg(2))?;
    let mut new_rows = Vec::with_capacity(xs.row_count());
    for x in xs.into_rows() {
        env.push(y.clone());
        env.push(x);
        env.call_error_on_break(f.clone(), "break is not allowed in distribute")?;
        new_rows.push(env.pop("distribute's function result")?);
    }
    env.push(Value::from_row_values(new_rows, env)?);
    Ok(())
}

fn bin_bool<T: ArrayValue>(f: impl Fn(T, T) -> bool + Copy) -> impl Fn(T, T) -> u8 {
    move |x, y| f(x, y) as u8
}

pub fn table(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let xs = env.pop(ArrayArg(1))?;
    let ys = env.pop(ArrayArg(2))?;
    match (f.as_flipped_primitive(), xs, ys) {
        (Some((prim, flipped)), Value::Num(xs), Value::Num(ys)) => {
            if let Err((xs, ys)) = table_nums(prim, flipped, xs, ys, env) {
                return generic_table(f, Value::Num(xs), Value::Num(ys), env);
            }
        }
        (Some((prim, flipped)), Value::Num(xs), Value::Byte(ys)) => {
            let ys = ys.convert();
            if let Err((xs, ys)) = table_nums(prim, flipped, xs, ys, env) {
                return generic_table(f, Value::Num(xs), Value::Num(ys), env);
            }
        }
        (Some((prim, flipped)), Value::Byte(xs), Value::Num(ys)) => {
            let xs = xs.convert();
            if let Err((xs, ys)) = table_nums(prim, flipped, xs, ys, env) {
                return generic_table(f, Value::Num(xs), Value::Num(ys), env);
            }
        }
        (Some((prim, flipped)), Value::Byte(xs), Value::Byte(ys)) => match prim {
            Primitive::Eq => env.push(fast_table(xs, ys, bin_bool(|x, y| x == y))),
            Primitive::Ne => env.push(fast_table(xs, ys, bin_bool(|x, y| x != y))),
            Primitive::Lt if flipped => env.push(fast_table(xs, ys, bin_bool(|x, y| x < y))),
            Primitive::Lt => env.push(fast_table(xs, ys, bin_bool(|x, y| y < x))),
            Primitive::Gt if flipped => env.push(fast_table(xs, ys, bin_bool(|x, y| x > y))),
            Primitive::Gt => env.push(fast_table(xs, ys, bin_bool(|x, y| y > x))),
            Primitive::Le if flipped => env.push(fast_table(xs, ys, bin_bool(|x, y| x <= y))),
            Primitive::Le => env.push(fast_table(xs, ys, bin_bool(|x, y| y <= x))),
            Primitive::Ge if flipped => env.push(fast_table(xs, ys, bin_bool(|x, y| x >= y))),
            Primitive::Ge => env.push(fast_table(xs, ys, bin_bool(|x, y| y >= x))),
            Primitive::Add => env.push(fast_table(xs, ys, |a, b| f64::from(a) + f64::from(b))),
            Primitive::Sub if flipped => {
                env.push(fast_table(xs, ys, |a, b| f64::from(a) - f64::from(b)))
            }
            Primitive::Sub => env.push(fast_table(xs, ys, |a, b| f64::from(b) - f64::from(a))),
            Primitive::Mul => env.push(fast_table(xs, ys, |a, b| f64::from(a) * f64::from(b))),
            Primitive::Div if flipped => {
                env.push(fast_table(xs, ys, |a, b| f64::from(a) / f64::from(b)))
            }
            Primitive::Div => env.push(fast_table(xs, ys, |a, b| f64::from(b) / f64::from(a))),
            Primitive::Min => env.push(fast_table(xs, ys, u8::min)),
            Primitive::Max => env.push(fast_table(xs, ys, u8::max)),
            Primitive::Join | Primitive::Couple => env.push(fast_table_join_or_couple(xs, ys)),
            _ => generic_table(f, Value::Byte(xs), Value::Byte(ys), env)?,
        },
        (_, xs, ys) => generic_table(f, xs, ys, env)?,
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn table_nums(
    prim: Primitive,
    flipped: bool,
    xs: Array<f64>,
    ys: Array<f64>,
    env: &mut Uiua,
) -> Result<(), (Array<f64>, Array<f64>)> {
    match prim {
        Primitive::Eq => env.push(fast_table(xs, ys, bin_bool(|x, y| x == y))),
        Primitive::Ne => env.push(fast_table(xs, ys, bin_bool(|x, y| x != y))),
        Primitive::Lt if flipped => env.push(fast_table(xs, ys, bin_bool(|x, y| x < y))),
        Primitive::Lt => env.push(fast_table(xs, ys, bin_bool(|x, y| y < x))),
        Primitive::Gt if flipped => env.push(fast_table(xs, ys, bin_bool(|x, y| x > y))),
        Primitive::Gt => env.push(fast_table(xs, ys, bin_bool(|x, y| y > x))),
        Primitive::Le if flipped => env.push(fast_table(xs, ys, bin_bool(|x, y| x <= y))),
        Primitive::Le => env.push(fast_table(xs, ys, bin_bool(|x, y| y <= x))),
        Primitive::Ge if flipped => env.push(fast_table(xs, ys, bin_bool(|x, y| x >= y))),
        Primitive::Ge => env.push(fast_table(xs, ys, bin_bool(|x, y| y >= x))),
        Primitive::Add => env.push(fast_table(xs, ys, Add::add)),
        Primitive::Sub if flipped => env.push(fast_table(xs, ys, Sub::sub)),
        Primitive::Sub => env.push(fast_table(xs, ys, flip(Sub::sub))),
        Primitive::Mul => env.push(fast_table(xs, ys, Mul::mul)),
        Primitive::Div if flipped => env.push(fast_table(xs, ys, Div::div)),
        Primitive::Div => env.push(fast_table(xs, ys, flip(Div::div))),
        Primitive::Min => env.push(fast_table(xs, ys, f64::min)),
        Primitive::Max => env.push(fast_table(xs, ys, f64::max)),
        Primitive::Join | Primitive::Couple => env.push(fast_table_join_or_couple(xs, ys)),
        _ => return Err((xs, ys)),
    }
    Ok(())
}

fn fast_table<A: ArrayValue, B: ArrayValue, C: ArrayValue>(
    a: Array<A>,
    b: Array<B>,
    f: impl Fn(A, B) -> C,
) -> Array<C> {
    let mut new_data = Vec::with_capacity(a.data.len() * b.data.len());
    for x in a.data {
        for y in b.data.iter().cloned() {
            new_data.push(f(x.clone(), y));
        }
    }
    let mut new_shape = a.shape;
    new_shape.extend_from_slice(&b.shape);
    Array::new(new_shape, new_data)
}

fn fast_table_join_or_couple<T: ArrayValue>(a: Array<T>, b: Array<T>) -> Array<T> {
    let mut new_data = Vec::with_capacity(a.data.len() * b.data.len() * 2);
    for x in a.data {
        for y in b.data.iter().cloned() {
            new_data.push(x.clone());
            new_data.push(y);
        }
    }
    let mut new_shape = a.shape;
    new_shape.extend_from_slice(&b.shape);
    new_shape.push(2);
    Array::new(new_shape, new_data)
}

fn generic_table(f: Value, xs: Value, ys: Value, env: &mut Uiua) -> UiuaResult {
    let mut new_shape = Shape::from(xs.shape());
    new_shape.extend_from_slice(ys.shape());
    let mut items = Vec::with_capacity(xs.flat_len() * ys.flat_len());
    let y_values = ys.into_flat_values().collect::<Vec<_>>();
    for x in xs.into_flat_values() {
        for y in y_values.iter().cloned() {
            env.push(y);
            env.push(x.clone());
            env.call_error_on_break(f.clone(), "break is not allowed in table")?;
            let item = env.pop("tabled function result")?;
            item.validate_shape();
            items.push(item);
        }
    }
    let mut tabled = Value::from_row_values(items, env)?;
    new_shape.extend_from_slice(&tabled.shape()[1..]);
    *tabled.shape_mut() = new_shape;
    tabled.validate_shape();
    env.push(tabled);
    Ok(())
}

pub fn cross(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let xs = env.pop(ArrayArg(1))?;
    let ys = env.pop(ArrayArg(2))?;
    let mut new_shape = tiny_vec![xs.row_count(), ys.row_count()];
    let mut items = Vec::with_capacity(xs.row_count() * ys.row_count());
    let y_rows = ys.into_rows().collect::<Vec<_>>();
    for x_row in xs.into_rows() {
        for y_row in y_rows.iter().cloned() {
            env.push(y_row);
            env.push(x_row.clone());
            env.call_error_on_break(f.clone(), "break is not allowed in cross")?;
            let item = env.pop("crossed function result")?;
            item.validate_shape();
            items.push(item);
        }
    }
    let mut crossed = Value::from_row_values(items, env)?;
    new_shape.extend_from_slice(&crossed.shape()[1..]);
    *crossed.shape_mut() = new_shape;
    crossed.validate_shape();
    env.push(crossed);
    Ok(())
}

pub fn repeat(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let n = env
        .pop(2)?
        .as_num(env, "Repetitions must be a single integer or infinity")?;

    if n.is_infinite() {
        let f = if n < 0.0 { f.invert(env)? } else { f };
        loop {
            if env.call_catch_break(f.clone())? {
                break;
            }
        }
    } else {
        if n.fract().abs() > f64::EPSILON {
            return Err(env.error("Repetitions must be a single integer or infinity"));
        };
        let f = if n < 0.0 { f.invert(env)? } else { f };
        for _ in 0..n.abs() as usize {
            if env.call_catch_break(f.clone())? {
                return Ok(());
            }
        }
    }
    Ok(())
}

pub fn level(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let get_ns = env.pop(FunctionArg(1))?;
    env.call_error_on_break(get_ns, "break is not allowed in level")?;
    let ns = env.pop("level's rank list")?.as_number_list(
        env,
        "Elements of rank list must be integers or infinity",
        |n| n.fract() == 0.0 || n == f64::INFINITY,
        |n| {
            if n == f64::INFINITY {
                None
            } else {
                Some(n as isize)
            }
        },
    )?;
    let f = env.pop(FunctionArg(2))?;
    match ns.as_slice() {
        [] => return Ok(()),
        &[n] => {
            let xs = env.pop(ArrayArg(1))?;
            if xs.rank() == 0 {
                env.push(xs);
                return Ok(());
            }
            let rank = match n {
                Some(0) => return each1_1(f, xs, env),
                Some(-1) => return rows1_1(f, xs, env),
                None => {
                    env.push(xs);
                    return env.call(f);
                }
                Some(n) => n,
            };
            let rank = if rank < 0 {
                (xs.rank() as isize + rank).max(0) as usize
            } else {
                (rank as usize).min(xs.rank())
            };
            let n = xs.rank() - rank;
            let res = monadic_level_recursive(f, xs, n, env)?;
            env.push(res);
        }
        &[xn, yn] => {
            let xs = env.pop(ArrayArg(1))?;
            let ys = env.pop(ArrayArg(2))?;
            if xs.rank() == 0 && ys.rank() == 0 {
                env.push(xs);
                env.push(ys);
                return Ok(());
            }
            match (xn, yn) {
                (Some(0), Some(0)) => return each2_1(f, xs, ys, env),
                (Some(-1), Some(-1)) => return rows2_1(f, xs, ys, env),
                (None, None) => {
                    env.push(ys);
                    env.push(xs);
                    return env.call(f);
                }
                _ => {}
            }
            let xn = xn.unwrap_or(xs.rank() as isize);
            let yn = yn.unwrap_or(ys.rank() as isize);
            let xn = if xn < 0 {
                (xs.rank() as isize + xn).max(0) as usize
            } else {
                (xn as usize).min(xs.rank())
            };
            let yn = if yn < 0 {
                (ys.rank() as isize + yn).max(0) as usize
            } else {
                (yn as usize).min(ys.rank())
            };
            let xn = xs.rank() - xn;
            let yn = ys.rank() - yn;
            let res = dyadic_level_recursive(f, xs, ys, xn, yn, env)?;
            env.push(res);
        }
        is => {
            let mut args = Vec::with_capacity(is.len());
            for i in 0..is.len() {
                let arg = env.pop(ArrayArg(i + 1))?;
                args.push(arg);
            }
            let mut ns: Vec<usize> = Vec::with_capacity(is.len());
            for (i, arg) in args.iter().enumerate() {
                let rank = is[i].unwrap_or(arg.rank() as isize);
                let rank = if rank < 0 {
                    (arg.rank() as isize + rank).max(0) as usize
                } else {
                    (rank as usize).min(arg.rank())
                };
                let n = arg.rank() - rank;
                ns.push(n);
            }
            let res = multi_level_recursive(f, args, &ns, env)?;
            env.push(res);
        }
    }
    Ok(())
}

fn monadic_level_recursive(f: Value, value: Value, n: usize, env: &mut Uiua) -> UiuaResult<Value> {
    if n == 0 {
        env.push(value);
        env.call(f)?;
        Ok(env.pop("level's function result")?)
    } else {
        let mut rows = Vec::with_capacity(value.row_count());
        for row in value.into_rows() {
            rows.push(monadic_level_recursive(f.clone(), row, n - 1, env)?);
        }
        Value::from_row_values(rows, env)
    }
}

fn dyadic_level_recursive(
    f: Value,
    xs: Value,
    ys: Value,
    xn: usize,
    yn: usize,
    env: &mut Uiua,
) -> UiuaResult<Value> {
    let xs_prefix = &xs.shape()[..xn];
    let ys_prefix = &ys.shape()[..yn];
    if !xs_prefix.iter().zip(ys_prefix).all(|(a, b)| a == b) {
        return Err(env.error(format!(
            "Cannot level with ranks {} and {} arrays with shapes {} and {} \
            because shape prefixes {} and {} are not compatible",
            xs.rank() - xn,
            ys.rank() - yn,
            xs.format_shape(),
            ys.format_shape(),
            FormatShape(xs_prefix),
            FormatShape(ys_prefix)
        )));
    }
    Ok(match (xn, yn) {
        (0, 0) => {
            env.push(ys);
            env.push(xs);
            env.call(f)?;
            env.pop("level's function result")?
        }
        (0, yn) => {
            let mut new_rows = Vec::with_capacity(ys.row_count());
            for y in ys.into_rows() {
                new_rows.push(dyadic_level_recursive(
                    f.clone(),
                    xs.clone(),
                    y,
                    xn,
                    yn - 1,
                    env,
                )?);
            }
            Value::from_row_values(new_rows, env)?
        }
        (xn, 0) => {
            let mut new_rows = Vec::with_capacity(xs.row_count());
            for x in xs.into_rows() {
                new_rows.push(dyadic_level_recursive(
                    f.clone(),
                    x,
                    ys.clone(),
                    xn - 1,
                    yn,
                    env,
                )?);
            }
            Value::from_row_values(new_rows, env)?
        }
        (xn, yn) => {
            let mut new_rows = Vec::with_capacity(xs.row_count());
            for (x, y) in xs.into_rows().zip(ys.into_rows()) {
                new_rows.push(dyadic_level_recursive(
                    f.clone(),
                    x,
                    y,
                    xn - 1,
                    yn - 1,
                    env,
                )?);
            }
            Value::from_row_values(new_rows, env)?
        }
    })
}

fn multi_level_recursive(
    f: Value,
    args: Vec<Value>,
    ns: &[usize],
    env: &mut Uiua,
) -> UiuaResult<Value> {
    if ns.iter().all(|&n| n == 0) {
        for arg in args.into_iter().rev() {
            env.push(arg);
        }
        env.call(f)?;
        Ok(env.pop("level's function result")?)
    } else {
        let (&n_with_max_row_count, arg_with_max_row_count) = ns
            .iter()
            .zip(&args)
            .max_by_key(|&(&n, v)| if n == 0 { 1 } else { v.shape()[0] })
            .unwrap();
        for (n, arg) in ns.iter().zip(&args) {
            if !arg.shape()[..*n]
                .iter()
                .zip(&arg_with_max_row_count.shape()[..n_with_max_row_count])
                .all(|(a, b)| a == b)
            {
                return Err(env.error(format!(
                    "Cannot level with ranks {} and {} arrays with shapes {} and {}",
                    arg_with_max_row_count.rank() - n_with_max_row_count,
                    arg.rank() - n,
                    arg_with_max_row_count.format_shape(),
                    arg.format_shape()
                )));
            }
        }
        let row_count = if n_with_max_row_count == 0 {
            1
        } else {
            arg_with_max_row_count.shape()[0]
        };
        let mut rows = Vec::with_capacity(row_count);
        let mut row_args = args.clone();
        let mut dec_ns = ns.to_vec();
        for n in dec_ns.iter_mut() {
            *n = n.saturating_sub(1);
        }
        for i in 0..row_count {
            for (j, (arg, n)) in args.iter().zip(ns).enumerate() {
                row_args[j] = if *n == 0 { arg.clone() } else { arg.row(i) };
            }
            let row = multi_level_recursive(f.clone(), row_args.clone(), &dec_ns, env)?;
            rows.push(row);
        }
        Value::from_row_values(rows, env)
    }
}

pub fn partition(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let markers = env.pop(ArrayArg(1))?;
    let markers = markers.as_indices(env, "Partition markers must be a list of integers")?;
    let values = env.pop(ArrayArg(2))?;
    let groups = values.partition_groups(&markers, env)?;
    collapse_groups(f, groups, "partition", env)
}

impl Value {
    pub fn partition_groups(&self, markers: &[isize], env: &Uiua) -> UiuaResult<Vec<Self>> {
        Ok(match self {
            Value::Num(arr) => arr
                .partition_groups(markers, env)?
                .map(Into::into)
                .collect(),
            Value::Byte(arr) => arr
                .partition_groups(markers, env)?
                .map(Into::into)
                .collect(),
            Value::Char(arr) => arr
                .partition_groups(markers, env)?
                .map(Into::into)
                .collect(),
            Value::Func(arr) => arr
                .partition_groups(markers, env)?
                .map(Into::into)
                .collect(),
        })
    }
}

impl<T: ArrayValue> Array<T> {
    pub fn partition_groups(
        &self,
        markers: &[isize],
        env: &Uiua,
    ) -> UiuaResult<impl Iterator<Item = Self>> {
        if markers.len() != self.row_count() {
            return Err(env.error(format!(
                "Cannot partition array of shape {} with markers of length {}",
                self.format_shape(),
                markers.len()
            )));
        }
        let mut groups = Vec::new();
        let mut last_marker = isize::MAX;
        for (row, &marker) in self.rows().zip(markers) {
            if marker > 0 {
                if marker != last_marker {
                    groups.push(Vec::new());
                }
                groups.last_mut().unwrap().push(row);
            }
            last_marker = marker;
        }
        Ok(groups
            .into_iter()
            .rev()
            .map(Array::from_row_arrays_infallible))
    }
}

pub fn group(env: &mut Uiua) -> UiuaResult {
    crate::profile_function!();
    let f = env.pop(FunctionArg(1))?;
    let indices = env.pop(ArrayArg(1))?;
    let indices = indices.as_indices(env, "Group indices must be a list of integers")?;
    let values = env.pop(ArrayArg(2))?;
    let groups = values.group_groups(&indices, env)?;
    collapse_groups(f, groups, "group", env)
}

impl Value {
    pub fn group_groups(&self, indices: &[isize], env: &Uiua) -> UiuaResult<Vec<Self>> {
        Ok(match self {
            Value::Num(arr) => arr.group_groups(indices, env)?.map(Into::into).collect(),
            Value::Byte(arr) => arr.group_groups(indices, env)?.map(Into::into).collect(),
            Value::Char(arr) => arr.group_groups(indices, env)?.map(Into::into).collect(),
            Value::Func(arr) => arr.group_groups(indices, env)?.map(Into::into).collect(),
        })
    }
}

impl<T: ArrayValue> Array<T> {
    pub fn group_groups(
        &self,
        indices: &[isize],
        env: &Uiua,
    ) -> UiuaResult<impl Iterator<Item = Self>> {
        if indices.len() != self.row_count() {
            return Err(env.error(format!(
                "Cannot group array of shape {} with indices of length {}",
                self.format_shape(),
                indices.len()
            )));
        }
        let Some(&max_index) = indices.iter().max() else {
            return Ok(Vec::<Vec<Self>>::new()
                .into_iter()
                .rev()
                .map(Array::from_row_arrays_infallible));
        };
        let mut groups: Vec<Vec<Self>> = vec![Vec::new(); max_index.max(0) as usize + 1];
        for (r, &g) in indices.iter().enumerate() {
            if g >= 0 && r < self.row_count() {
                groups[g as usize].push(self.row(r));
            }
        }
        Ok(groups
            .into_iter()
            .rev()
            .map(Array::from_row_arrays_infallible))
    }
}

fn collapse_groups<G>(f: Value, groups: G, name: &str, env: &mut Uiua) -> UiuaResult
where
    G: IntoIterator<Item = Value>,
    G::IntoIter: ExactSizeIterator,
{
    let mut groups = groups.into_iter();
    match f.signature().args {
        0 | 1 => {
            let mut rows = Vec::with_capacity(groups.len());
            for group in groups {
                env.push(group);
                env.call_error_on_break_with(f.clone(), || {
                    format!("break is not allowed in {name}")
                })?;
                rows.push(env.pop(|| format!("{name}'s function result"))?);
            }
            rows.reverse();
            let res = Value::from_row_values(rows, env)?;
            env.push(res);
        }
        2 => {
            let mut acc = groups
                .next()
                .ok_or_else(|| env.error(format!("Cannot reduce empty {name} result")))?;
            for row in groups {
                env.push(acc);
                env.push(row);
                if env.call_catch_break(f.clone())? {
                    return Ok(());
                }
                acc = env.pop("reduced function result")?;
            }
            env.push(acc);
        }
        args => {
            return Err(env.error(format!(
                "Cannot {name} with a function that takes {args} arguments"
            )))
        }
    }
    Ok(())
}
