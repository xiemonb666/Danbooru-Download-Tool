import os
from PIL import Image

def is_image_corrupted(image_path):
    if not os.path.isfile(image_path):  # 确认文件存在
        print(f"File does not exist: {image_path}")
        return True

    try:
        with Image.open(image_path) as img:
            img.verify()  # 验证图像文件是否完整，但不加载图像数据
            img = Image.open(image_path)  # 再次打开图像文件
            img.load()    # 加载图像数据，确保图像未损坏
        return False
    except (IOError, SyntaxError, AttributeError) as e:
        print(f"Image {image_path} is corrupted: {e}")
        return True

def remove_corrupted_images_and_texts(directory):
    deleted_count = 0  # 初始化计数器
    for filename in os.listdir(directory):
        file_path = os.path.join(directory, filename)
        if os.path.isfile(file_path):
            ext = os.path.splitext(filename)[1].lower()
            if ext in ['.jpg', '.jpeg', '.png', '.gif', '.bmp', '.tiff']:  # 添加你想要检查的图片格式
                if is_image_corrupted(file_path):
                    print(f"Removing corrupted image: {file_path}")
                    os.remove(file_path)  # 删除损坏的图片
                    deleted_count += 1  # 增加计数器

                    # 获取同名的txt文件路径并删除
                    txt_file_path = os.path.splitext(file_path)[0] + '.txt'
                    if os.path.exists(txt_file_path):
                        print(f"Removing associated text file: {txt_file_path}")
                        os.remove(txt_file_path)

    return deleted_count  # 返回删除的数量

if __name__ == '__main__':
    directory_to_check = r'danbooru_images\the_herta'  # 指定你要检查的目录路径
    deleted_images = remove_corrupted_images_and_texts(directory_to_check)
    print(f"Total number of deleted images: {deleted_images}")