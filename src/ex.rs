use std::time::Instant;
use RUSTUP_HOME;
use CARGO_HOME;
use std::env;
use std::fs;
use errors::*;
use EXPERIMENT_DIR;
use std::path::{Path, PathBuf};
use crates;
use lists::Crate;
use run;
use std::collections::HashMap;
use serde_json;
use file;
use toolchain;
use util;
use std::fmt::{self, Formatter, Display};
use log;
use toml_frobber;
use TEST_DIR;

fn ex_dir(ex_name: &str) -> PathBuf {
    Path::new(EXPERIMENT_DIR).join(ex_name)
}

fn shafile(ex_name: &str) -> PathBuf {
    Path::new(EXPERIMENT_DIR).join(ex_name).join("shas.json")
}

pub fn capture_shas(ex_name: &str) -> Result<()> {
    let mut shas: HashMap<String, String> = HashMap::new();
    for (krate, dir) in crates::crates_and_dirs()? {
        match krate {
            Crate::Repo(url) => {
                let r = run::run_capture(Some(&dir),
                                         "git",
                                         &["log", "-n1", "--pretty=%H"],
                                         &[]);

                match r {
                    Ok((stdout, stderr)) => {
                        if let Some(shaline) = stdout.get(0) {
                            if shaline.len() > 0 {
                                log!("sha for {}: {}", url, shaline);
                                shas.insert(url, shaline.to_string());
                            } else {
                                log_err!("bogus output from git log for {}", dir.display());
                            }
                        } else {
                            log_err!("bogus output from git log for {}", dir.display());
                        }
                    }
                    Err(e) => {
                        log_err!("unable to capture sha for {}: {}", dir.display(), e);
                    }
                }
            }
            _ => ()
        }
    }

    fs::create_dir_all(&ex_dir(ex_name))?;
    let shajson = serde_json::to_string(&shas)
        .chain_err(|| "unable to serialize json")?;
    file::write_string(&shafile(ex_name), &shajson)?;

    Ok(())
}

fn lockfile_dir(ex_name: &str) -> PathBuf {
    Path::new(EXPERIMENT_DIR).join(ex_name).join("lockfiles")
}

fn lockfile(ex_name: &str, crate_: &Crate) -> PathBuf {
    let (crate_name, crate_vers) = match *crate_ {
        Crate::Version(ref n, ref v) => (n.to_string(), v.to_string()),
        _ => panic!("unimplemented crate type in `lockfile`"),
    };
    lockfile_dir(ex_name).join(format!("{}-{}.lock", crate_name, crate_vers))
}

pub fn capture_lockfiles(ex_name: &str, toolchain: &str, recapture_existing: bool) -> Result<()> {
    fs::create_dir_all(&lockfile_dir(ex_name))?;

    let crates = crates::crates_and_dirs()?;

    for (ref c, ref dir) in crates {
        if dir.join("Cargo.lock").exists() {
            log!("crate {} has a lockfile. skipping", c);
            continue;
        }
        let captured_lockfile = lockfile(ex_name, c);
        if captured_lockfile.exists() && !recapture_existing {
            log!("skipping existing lockfile for {}", c);
            continue;
        }
        let r = crates::with_work_crate(c, |path| {
            with_frobbed_toml(c, path)?;
            capture_lockfile(ex_name, c, path, toolchain)
        }).chain_err(|| format!("failed to generate lockfile for {}", c));
        if let Err(e) = r {
            util::report_error(&e);
        }
    }

    Ok(())
}

fn capture_lockfile(ex_name: &str, crate_: &Crate, path: &Path, toolchain: &str) -> Result<()> {
    let manifest_path = path.join("Cargo.toml").to_string_lossy().to_string();
    let args = &["generate-lockfile",
                 "--manifest-path",
                 &*manifest_path];
    toolchain::run_cargo(toolchain, ex_name, args)
        .chain_err(|| format!("unable to generate lockfile for {}", crate_))?;

    let ref src_lockfile = path.join("Cargo.lock");
    let ref dst_lockfile = lockfile(ex_name, crate_);
    fs::copy(src_lockfile, dst_lockfile)
        .chain_err(|| format!("unable to copy lockfile from {} to {}",
                              src_lockfile.display(), dst_lockfile.display()))?;

    log!("generated lockfile for {} at {}", crate_, dst_lockfile.display());
    
    Ok(())
}

fn with_captured_lockfile(ex_name: &str, crate_: &Crate, path: &Path) -> Result<()> {
    let ref src_lockfile = lockfile(ex_name, crate_);
    let ref dst_lockfile = path.join("Cargo.lock");
    if src_lockfile.exists() {
        log!("using lockfile {}", src_lockfile.display());
        fs::copy(src_lockfile, dst_lockfile)
            .chain_err(|| format!("unable to copy lockfile from {} to {}",
                                  src_lockfile.display(), dst_lockfile.display()))?;
    }

    Ok(())
}

pub fn fetch_deps(ex_name: &str, toolchain: &str) -> Result<()> {
    let crates = crates::crates_and_dirs()?;
    for (ref c, ref dir) in crates {
        if dir.join("Cargo.lock").exists() {
            log!("crate {} has a lockfile. skipping", c);
            continue;
        }
        let r = crates::with_work_crate(c, |path| {
            with_frobbed_toml(c, path)?;
            with_captured_lockfile(ex_name, c, path)?;

            let manifest_path = path.join("Cargo.toml").to_string_lossy().to_string();
            let args = &["fetch",
                         "--locked",
                         "--manifest-path",
                         &*manifest_path];
            toolchain::run_cargo(toolchain, ex_name, args)
                .chain_err(|| format!("unable to fetch deps for {}", c))?;

            Ok(())
        });
        if let Err(e) = r {
            util::report_error(&e);
        }
    }

    Ok(())

}

fn with_frobbed_toml(crate_: &Crate, path: &Path) -> Result<()> {
    let (crate_name, crate_vers) = match *crate_ {
        Crate::Version(ref n, ref v) => (n.to_string(), v.to_string()),
        _ => panic!("unimplemented crate type in with_captured_lockfile"),
    };
    let ref src_froml = toml_frobber::froml_path(&crate_name, &crate_vers);
    let ref dst_froml = path.join("Cargo.toml");
    if src_froml.exists() {
        log!("using frobbed toml {}", src_froml.display());
        fs::copy(src_froml, dst_froml)
            .chain_err(|| format!("unable to copy frobbed toml from {} to {}",
                                  src_froml.display(), dst_froml.display()))?;
    }

    Ok(())
}

pub fn run_test(ex_name: &str, toolchain: &str) -> Result<()> {
    let crates = crates::crates_and_dirs()?;

    // Just for reporting progress
    let total_crates = crates.len();
    let mut skipped_crates = 0;
    let mut completed_crates = 0;

    // These should add up to total_crates
    let mut sum_errors = 0;
    let mut sum_fail = 0;
    let mut sum_build_pass = 0;
    let mut sum_test_pass = 0;

    let start_time = Instant::now();

    log!("running {} tests", total_crates);
    for (ref c, ref dir) in crates {
        if dir.join("Cargo.lock").exists() {
            log!("crate {} has a lockfile. skipping", c);
            continue;
        }
        let existing_result = get_test_result(ex_name, c, toolchain)?;
        if let Some(r) = existing_result {
            log!("skipping crate {}. existing result: {}", c, r);
            log!("delete result file to rerun test: {}",
                 result_file(ex_name, c, toolchain)?.display());
            skipped_crates += 1;
            continue;
        }

        // SCARY HACK: Crates in the container are built in the mounted
        // ./work/test directory. Some of them write files to that directory
        // which end up being owned by root. This command deletes those
        // files by running "rm" in the container.
        let test_dir = Path::new(TEST_DIR);
        if test_dir.exists() {
            let _ = run_in_docker(ex_name, test_dir,
                                  &["sh", "-c", "rm -r /test/*"]);
        }

        let r = crates::with_work_crate(c, |path| {
            with_frobbed_toml(c, path)?;
            with_captured_lockfile(ex_name, c, path)?;

            run_single_test(ex_name, c, path, toolchain)
        });

        match r {
            Err(ref e) => {
                log_err!("error testing crate {}:  {}", c, e);
                util::report_error(e);
            }
            Ok(ref r) => {
                // FIXME: Should errors be recorded?
                record_test_result(ex_name, c, toolchain, *r);
            }
        }

        match r {
            Err(_) => {
                sum_errors += 1;
            }
            Ok(TestResult::Fail) => sum_fail += 1,
            Ok(TestResult::BuildPass) => sum_build_pass +=1,
            Ok(TestResult::TestPass) => sum_test_pass += 1,
        }

        completed_crates += 1;

        let elapsed = Instant::now().duration_since(start_time).as_secs();
        let tests_per_second = (completed_crates as f64) / (elapsed as f64);
        let remaining_tests = total_crates - skipped_crates - completed_crates;
        let remaining_time = (remaining_tests as f64) * tests_per_second;

        log!("progress: {} / {} ({} skipped of {}) tests in {}s",
             completed_crates,
             total_crates - skipped_crates,
             skipped_crates,
             total_crates,
             elapsed);
        log!("{} tests/s. expecting {}s remaining", remaining_tests, remaining_time);
        log!("results: {} fail / {} build-pass / {} test-pass / {} errors",
             sum_fail, sum_build_pass, sum_test_pass, sum_errors);
    }

    Ok(())
}

#[derive(Copy, Clone)]
enum TestResult {
    Fail,
    BuildPass,
    TestPass,
}

impl Display for TestResult {
    fn fmt(&self, f: &mut Formatter) -> ::std::result::Result<(), fmt::Error> {
        self.to_string().fmt(f)
    }
}

impl TestResult {
    fn from_str(s: &str) -> Result<TestResult> {
        match s {
            "fail" => Ok(TestResult::Fail),
            "build-pass" => Ok(TestResult::BuildPass),
            "test-pass" => Ok(TestResult::TestPass),
            _ => Err(format!("bogus test result: {}", s).into())
        }
    }
    fn to_string(&self) -> String {
        match *self {
            TestResult::Fail => "fail",
            TestResult::BuildPass => "build-pass",
            TestResult::TestPass => "test-pass",
        }.to_string()
    }
}

fn run_single_test(ex_name: &str, c: &Crate, path: &Path, toolchain: &str) -> Result<TestResult> {

    let result_dir = result_dir(ex_name, c, toolchain)?;
    if result_dir.exists() {
        util::remove_dir_all(&result_dir)?;
    }
    fs::create_dir_all(&result_dir)?;
    let log_file = result_dir.join("log.txt");

    log::redirect(&log_file, || {
        let tc = toolchain::rustup_toolchain_name(toolchain)?;
        let tc_arg = &format!("+{}", tc);
        let build_r = run_in_docker(ex_name, path, &["cargo", tc_arg, "build", "--frozen"]);
        let test_r;

        if build_r.is_ok() {
            test_r = Some(run_in_docker(ex_name, path, &["cargo", tc_arg, "test", "--frozen"]));
        } else {
            test_r = None;
        }

        Ok(match (build_r, test_r) {
            (Err(_), None) => TestResult::Fail,
            (Ok(_), Some(Err(_))) => TestResult::BuildPass,
            (Ok(_), Some(Ok(_))) => TestResult::TestPass,
            (_, _) => unreachable!()
        })
    })
}

fn run_in_docker(ex_name: &str, path: &Path, args: &[&str]) -> Result<()> {

    let test_dir=absolute(path);
    let cargo_home=absolute(Path::new(CARGO_HOME));
    let rustup_home=absolute(Path::new(RUSTUP_HOME));
    let target_dir=absolute(&toolchain::target_dir(ex_name));

    fs::create_dir_all(&test_dir);
    fs::create_dir_all(&cargo_home);
    fs::create_dir_all(&rustup_home);
    fs::create_dir_all(&target_dir);

    let test_mount = &format!("{}:/test", test_dir.display());
    // FIXME this should be read-only https://github.com/rust-lang/cargo/issues/3256
    let cargo_home_mount = &format!("{}:/cargo-home", cargo_home.display());
    let rustup_home_mount = &format!("{}:/rustup-home:ro", rustup_home.display());
    let target_mount = &format!("{}:/target", target_dir.display());

    let image_name = "cargobomb";

    let mut args_ = vec!["run", "-i",
                         "--rm",
                         "-v", test_mount,
                         "-v", cargo_home_mount,
                         "-v", rustup_home_mount,
                         "-v", target_mount,
                         image_name];

    args_.extend_from_slice(args);

    run::run("docker", &args_, &[])
        .chain_err(|| "cargo build failed")?;

    Ok(())
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        let cd = env::current_dir().expect("unable to get current dir");
        cd.join(path)
    }
}

fn result_dir(ex_name: &str, c: &Crate, toolchain: &str) -> Result<PathBuf> {
    let tc = toolchain::rustup_toolchain_name(toolchain)?;
    Ok(ex_dir(ex_name).join("res").join(tc).join(crate_to_dir(c)))
}

fn crate_to_dir(c: &Crate) -> String {
    match *c {
        Crate::Version(ref n, ref v) => format!("crate/{}-{}", n, v),
        Crate::Repo(ref url) => {
            panic!()
        }
    }
}

fn result_file(ex_name: &str, c: &Crate, toolchain: &str) -> Result<PathBuf> {
    Ok(result_dir(ex_name, c, toolchain)?.join("results.txt"))
}

fn record_test_result(ex_name: &str, c: &Crate, toolchain: &str, r: TestResult) -> Result<()> {
    let result_dir = result_dir(ex_name, c, toolchain)?;
    fs::create_dir_all(&result_dir)?;
    let result_file = result_file(ex_name, c, toolchain)?;
    log!("test result! ex: {}, c: {}, tc: {}, r: {}", ex_name, c, toolchain, r);
    log!("file: {}", result_file.display());
    file::write_string(&result_file, &r.to_string())?;
    Ok(())
}

fn get_test_result(ex_name: &str, c: &Crate, toolchain: &str) -> Result<Option<TestResult>> {
    let result_file = result_file(ex_name, c, toolchain)?;
    if result_file.exists() {
        let s = file::read_string(&result_file)?;
        let r = TestResult::from_str(&s)
            .chain_err(|| format!("invalid test result value: '{}'", s))?;
        Ok(Some(r))
    } else {
        Ok(None)
    }
}
