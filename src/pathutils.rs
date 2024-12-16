use object_store::path::Path;

#[allow(dead_code)]
pub(crate) fn path_tail(a: Path, b: Path) -> Path {
    Path::from_iter(filter_tail_iterator(a.parts(), b.parts()))
}

#[allow(unused_variables,unused_mut,dead_code)]
pub fn filter_tail_iterator<T, I1, I2>(source: I1, mut comparison: I2) -> impl Iterator<Item = T>
where
    T: PartialEq + Clone,
    I1: Iterator<Item = T>,
    I2: Iterator<Item = T>,
{
    // source.zip_eq()
    // source.zip_longest(comparison)
    //     .skip_while(|&f| {
    //
    //
    //         let x = f.left();
    //         let y = f.right();
    //         x == y
    //     })
    //
    //
    //
    // source.filter_map(move |a| {
    //     match comparison.next() {
    //         Some(b) => {
    //             if matched {
    //                 return Some(a)
    //             }
    //
    //
    //         }
    //         None => Some(a),
    //     }
    // })
   source
}
