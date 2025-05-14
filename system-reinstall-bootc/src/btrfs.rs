use anyhow::Result;
use bootc_mount::Filesystem;

pub(crate) fn check_root_siblings() -> Result<Vec<String>> {
    let mounts = bootc_mount::run_findmnt(&[], None)?;
    let problem_filesystems: Vec<String> = mounts
        .filesystems
        .iter()
        .filter(|fs| fs.target == "/")
        .flat_map(|root| {
            let children: Vec<&Filesystem> = root
                .children
                .iter()
                .flatten()
                .filter(|child| child.source == root.source)
                .collect();
            children
        })
        .map(|zs| {
            format!(
                "Type: {}, Mount Point: {}, Source: {}",
                zs.fstype, zs.target, zs.source
            )
        })
        .collect();
    Ok(problem_filesystems)
}
