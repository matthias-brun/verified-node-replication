#[allow(unused_imports)]
use builtin::*;
use builtin_macros::*;

mod pervasive;
mod spec;

use crate::pervasive::*;
use crate::pervasive::vec::*;
use crate::pervasive::modes::*;
use crate::pervasive::multiset::*;
use crate::pervasive::map::*;
use crate::pervasive::seq::*;
use crate::pervasive::set::*;
use crate::pervasive::option::*;
use crate::pervasive::atomic_ghost::*;
use crate::pervasive::cell::{PCell, PermissionOpt};
use crate::pervasive::result::Result;
use state_machines_macros::tokenized_state_machine;

use spec::rwlock::*;

verus_old_todo_no_ghost_blocks!{

/// a simpe cache padding for the struct fields
#[repr(align(128))]
pub struct CachePadded<T>(pub T);

#[verifier(external_body)] /* vattr */
pub fn spin_loop_hint() {
    core::hint::spin_loop();
}


////////////////////////////////////////////////////////////////////////////////////////////////////
// RwLockWriteGuard
////////////////////////////////////////////////////////////////////////////////////////////////////

/// structure used to release the exclusive write access of a lock when dropped.
//
/// This structure is created by the write and try_write methods on RwLockSpec.
tracked struct RwLockWriteGuard<T> {
    handle: Tracked<RwLockSpec::writer<PermissionOpt<T>>>,
    perm: Tracked<PermissionOpt<T>>,
    //foo: Tracked<T>,
    // rw_lock : Ghost<DistRwLock::Instance<T>>,
}

////////////////////////////////////////////////////////////////////////////////////////////////////
//  RwLockReadGuard
////////////////////////////////////////////////////////////////////////////////////////////////////

/// RAII structure used to release the shared read access of a lock when dropped.
///
/// This structure is created by the read and try_read methods on RwLockSpec.
tracked struct RwLockReadGuard<T> {
    handle: Tracked<RwLockSpec::reader<PermissionOpt<T>>>,
    // rw_lock : Ghost<DistRwLock::Instance<T>>,
}

impl<T> RwLockReadGuard<T> {
    spec fn view(&self) -> T {
        self.handle@@.key.view().value.get_Some_0()
        //self.handle@@.key.get_Some_0()

    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////
// RwLockSpec
////////////////////////////////////////////////////////////////////////////////////////////////////


struct_with_invariants!{
/// A reader-writer lock
struct RwLock<#[verifier(maybe_negative)] T> {
    /// cell containing the data
    cell: PCell<T>,
    /// exclusive access
    exc: CachePadded<AtomicBool<_, RwLockSpec::flag_exc<PermissionOpt<T>>, _>>,
    /// reference count
    rc: CachePadded<AtomicU64<_, RwLockSpec::flag_rc<PermissionOpt<T>>, _>>,

    /// the state machien instance
    tracked inst: RwLockSpec::Instance<PermissionOpt<T>>,
    ghost user_inv: Set<T>,
}

pub closed spec fn wf(&self) -> bool {
    predicate {
        forall |v: PermissionOpt<T>| #[trigger] self.inst.user_inv().contains(v) == (
            equal(v@.pcell, self.cell.id()) && v@.value.is_Some()
                && self.user_inv.contains(v@.value.get_Some_0())
        )
    }

    invariant on exc with (inst) specifically (self.exc.0) is (v: bool, g: RwLockSpec::flag_exc<PermissionOpt<T>>) {
        // g@ === RwLockSpec::token! [ inst => exc ==> v ]
        g@.instance == inst && g@.value == v
    }

    invariant on rc with (inst) specifically (self.rc.0) is (v: u64, g: RwLockSpec::flag_rc<PermissionOpt<T>>) {
        // g@ === RwLockSpec::token! [ inst => rc ==> v ]
        g@.instance == inst && g@.value == v as nat
    }
}
} // struct_with_invariants!


impl<T> RwLock<T> {
    /// Invariant on the data
    spec fn inv(&self, t: T) -> bool {
        self.user_inv.contains(t)
    }

    spec fn wf_write_handle(&self, write_handle: &RwLockWriteGuard<T>) -> bool {
        &&& write_handle.handle@@.instance == self.inst

        &&& write_handle.perm@@.pcell == self.cell.id()
        &&& write_handle.perm@@.value.is_None()
    }

    spec fn wf_read_handle(&self, read_handle: &RwLockReadGuard<T>) -> bool {
        &&& read_handle.handle@@.instance == self.inst

        &&& read_handle.handle@@.key.view().value.is_Some()
        &&& read_handle.handle@@.key.view().pcell == self.cell.id()
        &&& read_handle.handle@@.count == 1
    }

    fn new(t: T, inv: Ghost<FnSpec(T) -> bool>) -> (res: Self)
        requires
            inv@(t)
        ensures
            res.wf(),
            forall |v: T| res.inv(v) == inv@(v)
    {
        let (cell, perm) = PCell::<T>::new(t);

        let ghost set_inv = Set::new(inv@);

        let ghost user_inv = Set::new(closure_to_fn_spec(|s: PermissionOpt<T>| {
            &&& equal(s@.pcell, cell.id())
            &&& s@.value.is_Some()
            &&& set_inv.contains(s@.value.get_Some_0())
        }));

        let tracked inst;
        let tracked flag_exc;
        let tracked flag_rc;
        proof {
            let tracked (Trk(_inst), Trk(_flag_exc), Trk(_flag_rc), _, _, _, _) =
                RwLockSpec::Instance::<PermissionOpt<T>>::initialize_full(user_inv, perm@, Option::Some(perm.get()));
            inst = _inst;
            flag_exc = _flag_exc;
            flag_rc = _flag_rc;
        }

        let exc = AtomicBool::new(inst, false, flag_exc);
        let rc = AtomicU64::new(inst, 0, flag_rc);

        RwLock { cell, exc: CachePadded(exc), rc: CachePadded(rc), inst, user_inv: set_inv }
    }

    fn acquire_write(&self) -> (res: (T, Tracked<RwLockWriteGuard<T>>))
        requires self.wf()
        ensures self.wf_write_handle(&res.1@) && self.inv(res.0)
    {

        let mut done = false;
        let tracked mut token: Option<RwLockSpec::pending_writer<PermissionOpt<T>>> = Option::None;
        while !done
            invariant
                self.wf(),
                done ==> token.is_Some() && token.get_Some_0()@.instance == self.inst,
        {
            let result = atomic_with_ghost!(
                &self.exc.0 => compare_exchange(false, true);
                returning res;
                ghost g =>
            {
                if res.is_Ok() {
                    token = Option::Some(self.inst.acquire_exc_start(&mut g));
                }
            });

            done = match result {
                Result::Ok(_) => true,
                _ => false,
            };
        }

        loop
            invariant
                self.wf(),
                token.is_Some() && token.get_Some_0()@.instance == self.inst,
        {

            let tracked mut perm_opt: Option<PermissionOpt<T>> = Option::None;
            let tracked mut handle_opt: Option<RwLockSpec::writer<PermissionOpt<T>>> =Option::None;
            let tracked acquire_exc_end_result; // need to define tracked, can't in the body
            let result = atomic_with_ghost!(
                &self.rc.0 => load();
                returning res;
                ghost g => {
                if res == 0 {
                    acquire_exc_end_result = self.inst.acquire_exc_end(&g, token.tracked_unwrap());
                    token = Option::None;
                    perm_opt = Option::Some(acquire_exc_end_result.1.0);
                    handle_opt = Option::Some(acquire_exc_end_result.2.0);
                }
            });

            if result == 0 {
                let mut perm = tracked(perm_opt.tracked_unwrap());
                let tracked handle = tracked(handle_opt.tracked_unwrap());

                let t = self.cell.take(&mut perm);

                let tracked write_handle = RwLockWriteGuard { perm, handle };
                return (t, tracked(write_handle));
            }
        }
    }

    fn acquire_read(&self) -> (res: Tracked<RwLockReadGuard<T>>)
        requires self.wf()
        ensures
            self.wf_read_handle(&res@) && self.inv(res@@)
    {
        loop
            invariant self.wf()
        {

            let val = atomic_with_ghost!(&self.rc.0 => load(); ghost g => { });

            let tracked mut token: Option<RwLockSpec::pending_reader<PermissionOpt<T>>> = Option::None;

            if val < 18446744073709551615 {
                let result = atomic_with_ghost!(
                    &self.rc.0 => compare_exchange(val, val + 1);
                    returning res;
                    ghost g =>
                {
                    if res.is_Ok() {
                        token = Option::Some(self.inst.acquire_read_start(&mut g));
                    }
                });

                match result {
                    Result::Ok(_) => {
                        let tracked mut handle_opt: Option<RwLockSpec::reader<PermissionOpt<T>>> = Option::None;
                        let tracked acquire_read_end_result;
                        let result = atomic_with_ghost!(
                            &self.exc.0 => load();
                            returning res;
                            ghost g =>
                        {
                            if res == false {
                                acquire_read_end_result = self.inst.acquire_read_end(&g, token.tracked_unwrap());
                                token = Option::None;
                                handle_opt = Option::Some(acquire_read_end_result.1.0);
                            }
                        });

                        if result == false {
                            let tracked handle = tracked(handle_opt.tracked_unwrap());
                            return tracked(RwLockReadGuard { handle });
                        } else {
                            let _ = atomic_with_ghost!(
                                &self.rc.0 => fetch_sub(1);
                                ghost g =>
                            {
                                self.inst.acquire_read_abandon(&mut g, token.tracked_unwrap());
                            });
                        }
                    }
                    _ => { }
                }
            }
        }
    }

    fn borrow<'a>(&'a self, read_handle: &'a Tracked<RwLockReadGuard<T>>) -> (res: &'a T)
        requires self.wf() && self.wf_read_handle(&read_handle@)
        ensures res == read_handle@@
    {
        let tracked perm = self.inst.read_guard(read_handle@.handle@@.key, read_handle.borrow().handle.borrow());
        self.cell.borrow(tracked_exec_borrow(perm))
    }

    fn release_write(&self, t: T, write_handle: Tracked<RwLockWriteGuard<T>>)
        requires
            self.wf() && self.wf_write_handle(&write_handle@) && self.inv(t)
    {
        let tracked write_handle = write_handle.get();
        let mut perm = tracked_exec(write_handle.perm.get());

        self.cell.put(&mut perm, t);

        atomic_with_ghost!(
            &self.exc.0 => store(false);
            ghost g =>
        {
            self.inst.release_exc(perm.view(), &mut g, perm.get(), write_handle.handle.get());
        });
    }

    fn release_read(&self, read_handle: Tracked<RwLockReadGuard<T>>)
        requires self.wf() && self.wf_read_handle(&read_handle@)
    {
        let tracked  RwLockReadGuard { handle } = read_handle.get();

        let _ = atomic_with_ghost!(
            &self.rc.0 => fetch_sub(1);
            ghost g =>
        {
            self.inst.release_shared(handle.view().view().key, &mut g, handle.get());
        });
    }

    proof fn lemma_readers_match(tracked &self, tracked read_handle1: &Tracked<RwLockReadGuard<T>>, tracked read_handle2: &Tracked<RwLockReadGuard<T>>)
        requires
            self.wf() && self.wf_read_handle(&read_handle1@) && self.wf_read_handle(&read_handle2@)
        ensures
            read_handle1@@ == read_handle2@@
    {
        self.inst.read_match(
            read_handle1@.handle@@.key,
            read_handle2@.handle@@.key,
            read_handle1.borrow().handle.borrow(),
            read_handle2.borrow().handle.borrow());
    }
}

fn main() {
    let ghost inv = closure_to_fn_spec(|v: u64| v == 5 || v == 13);
    let lock = RwLock::<u64>::new(5, ghost(inv));

    let (val, write_handle) = lock.acquire_write();
    assert(val == 5 || val == 13);
    lock.release_write(13, write_handle);

    let read_handle1 = lock.acquire_read();
    let read_handle2 = lock.acquire_read();

    let val1 = lock.borrow(&read_handle1);
    let val2 = lock.borrow(&read_handle2);

    lock.lemma_readers_match(&read_handle1, &read_handle2);
    assert(*val1 == *val2);

    lock.release_read(read_handle1);
    lock.release_read(read_handle2);
}

} // verus!


