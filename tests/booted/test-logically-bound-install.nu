use std assert
use tap.nu

# This list reflects the LBIs specified in bootc/tests/containerfiles/lbi/usr/share/containers/systemd
let expected_images = [
    "quay.io/curl/curl:latest",
    "quay.io/curl/curl-base:latest",
    "registry.access.redhat.com/ubi9/podman:latest" # this image is signed
]

def validate_images [images: list<string>] {
    print $"Validating images ($images)"
    for expected in $expected_images {
        assert ($expected in $images)
    }
}

# This test checks that bootc actually populated the bootc storage with the LBI images
def test_logically_bound_images_in_storage [] {
    # Use podman to list the images in the bootc storage
    let images = podman --storage-opt=additionalimagestore=/usr/lib/bootc/storage images --format {{.Repository}}:{{.Tag}} | split row "\n"

    # Debug print
    print "IMAGES:"
    podman --storage-opt=additionalimagestore=/usr/lib/bootc/storage images

    validate_images $images
}

# This test makes sure that bootc itself knows how to list the LBI images in the bootc storage
def test_bootc_image_list [] {
    # Use bootc to list the images in the bootc storage
    let images = bootc image list --type logical --format json | from json | get image

    validate_images $images
}

def test_storage_labels [] {
    let root_labeled = getfattr -n security.selinux /var/lib/containers/storage | grep container_var_lib_t | complete
    assert equal $root_labeled.exit_code 0
    let overlay_labeled = getfattr -n security.selinux /usr/lib/bootc/storage/overlay | grep container_ro_file_t | complete
    assert equal $overlay_labeled.exit_code 0
}

test_logically_bound_images_in_storage
test_bootc_image_list
test_storage_labels

tap ok
