# Start the node-based show editor (default task).
showui:
    cargo run --release -p scareflow-showui -- showui.toml

# Cross-compile showui for x86_64 Linux and copy it with the audio to a Linux box.
# Leaves the show file there alone, so edits made on that machine are kept; make
# a first one with scripts/generate-osc-show.py.
deploy-eris host="sixjes@eris.local":
    scripts/build-showui-linux.sh
    ssh {{host}} 'mkdir -p ~/scareflow'
    COPYFILE_DISABLE=1 tar -c --no-xattrs --exclude originals --exclude CREDITS.md scare bg-music -C target/linux-docker/x86_64-unknown-linux-gnu/release scareflow-showui -C ../../../../packaging/linux run.sh | ssh {{host}} 'tar -x -C ~/scareflow'
