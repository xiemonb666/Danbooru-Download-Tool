import os
import hashlib
from PIL import Image, UnidentifiedImageError
import warnings

# 忽略TIFF图像中损坏的EXIF数据警告
warnings.filterwarnings("ignore", message="Corrupt EXIF data.*", category=UserWarning, module='PIL.TiffImagePlugin')


def calculate_hash(image_path):
    """Calculate the hash of an image file."""
    try:
        with Image.open(image_path) as img:
            # Convert image to grayscale and resize it for faster hashing
            img = img.convert('L').resize((10, 10))
            # Calculate the hash value of the image
            hash_md5 = hashlib.md5(img.tobytes())
            return hash_md5.hexdigest()
    except UnidentifiedImageError:
        print(f"Unidentified image error when processing: {image_path}")
        return None
    except Exception as e:
        print(f"Error processing {image_path}: {e}")
        return None


def remove_duplicate_images_and_txt(folder_path):
    """Remove duplicate images within a folder and also delete corresponding .txt files.
       Returns the number of deleted images."""
    image_hashes = {}
    deleted_count = 0
    kept_count = 0

    for filename in sorted(os.listdir(folder_path)):  # 对文件名排序以确保按字母顺序处理
        file_path = os.path.join(folder_path, filename)

        # Only process image files
        if not os.path.isfile(file_path) or not filename.lower().endswith(('.png', '.jpg', '.jpeg', '.bmp', '.gif')):
            continue

        image_hash = calculate_hash(file_path)
        if image_hash is None:
            continue

        if image_hash in image_hashes:
            # This is a duplicate image, so we will delete it along with its .txt counterpart
            print(f"Deleting duplicate: {filename}")
            os.remove(file_path)  # Delete duplicate image
            deleted_count += 1

            # Try to delete the corresponding .txt file if it exists
            txt_file_path = os.path.splitext(file_path)[0] + '.txt'
            if os.path.exists(txt_file_path):
                print(f"Deleting corresponding txt: {txt_file_path}")
                os.remove(txt_file_path)
        else:
            # First time encountering this hash, keep the image
            image_hashes[image_hash] = file_path
            kept_count += 1

    return deleted_count, f"Process completed. Kept {kept_count} unique images and deleted {deleted_count} duplicates."   # 返回删除的图片数量


if __name__ == "__main__":
    folder_path = r'danbooru_images\the_herta'
    deleted_count, message = remove_duplicate_images_and_txt(folder_path)
    print(message)