export interface Ok<T> {
  Ok: T
}

export interface Err<T> {
  Err: T
}

export type Result<T, E> = Ok<T> | Err<E>

// carries the candid error variant alongside the message, so `errMessage` can
// render what the canister actually rejected with instead of a generic string
export class ErrData<T> extends Error {
  data?: T
  static from(msg: string, data?: any) {
    const err = new ErrData(msg)
    if (data) {
      err.data = data
    }
    return err
  }
}

export function unwrapResult<T, E>(
  res: Result<T, E>,
  msg: string = 'error result'
): T {
  if ('Err' in res) {
    throw ErrData.from(msg, res.Err)
  }

  return res.Ok
}
